import type { DistributedReplica, ReplicaWatch } from '../replica/index.js';
import type { GraphqlVariables } from '../types.js';
import type {
	DistributedBoundaryOperation,
	DistributedBoundaryVariableContext
} from './boundary-variables.js';

const MAX_BOUNDARY_INSTANCES = 4_096;
const MAX_INSTANCE_ID_BYTES = 512;

export type DistributedSvelteKitBoundaryInstance = Readonly<{
	/** Opaque identity for one mounted SvelteKit page or layout instance. */
	id: string;
	route: string;
	kind: 'layout' | 'page';
}>;

export type SveltekitBoundaryLifecycleDiagnostic = Readonly<{
	action:
		| 'acquire'
		| 'retain'
		| 'release'
		| 'scope-dispose'
		| 'final-unsubscribe';
	boundary: string;
	operation?: string;
	live?: boolean;
	owners: number;
}>;

export type SveltekitBoundaryRetention = Readonly<{
	release(): void;
}>;

type ResolvedBoundary = Readonly<{
	operation: DistributedBoundaryOperation;
	variables: GraphqlVariables;
	identity: string;
	live: boolean;
}>;

type RetainedBoundary = {
	signature: string;
	boundary: string;
	owners: number;
	watches: readonly Readonly<{
		watch: ReplicaWatch<unknown>;
		identity: string;
		operation: string;
		live: boolean;
	}>[];
};

export class DistributedSvelteKitBoundaryController {
	readonly #replica: DistributedReplica;
	readonly #operations: readonly DistributedBoundaryOperation[];
	readonly #diagnostic?: (event: SveltekitBoundaryLifecycleDiagnostic) => void;
	readonly #instances = new Map<string, RetainedBoundary>();
	readonly #identityOwners = new Map<string, number>();
	#destroyed = false;

	constructor(
		replica: DistributedReplica,
		operations: readonly DistributedBoundaryOperation[],
		diagnostic?: (event: SveltekitBoundaryLifecycleDiagnostic) => void
	) {
		this.#replica = replica;
		this.#operations = operations;
		this.#diagnostic = diagnostic;
	}

	retain<TSession, TProps>(
		instance: DistributedSvelteKitBoundaryInstance,
		context: DistributedBoundaryVariableContext<TSession, TProps>
	): SveltekitBoundaryRetention {
		if (this.#destroyed) {
			throw new Error('Distributed SvelteKit boundary controller is destroyed');
		}
		const validated = validateInstance(instance);
		const resolved = this.#resolve(validated, context);
		const signature = JSON.stringify(
			resolved.map(({ identity }) => identity)
		);
		const existing = this.#instances.get(validated.id);
		if (existing !== undefined) {
			if (
				existing.signature !== signature ||
				existing.boundary !== validated.boundary
			) {
				throw new Error(
					'Distributed SvelteKit boundary instance changed ownership while retained'
				);
			}
			existing.owners += 1;
			this.#emit({
				action: 'retain',
				boundary: existing.boundary,
				owners: existing.owners
			});
			return this.#lease(validated.id, existing);
		}
		if (this.#instances.size >= MAX_BOUNDARY_INSTANCES) {
			throw new Error(
				`Distributed SvelteKit cannot retain more than ${MAX_BOUNDARY_INSTANCES} boundary instances`
			);
		}

		const watches: Array<RetainedBoundary['watches'][number]> = [];
		try {
			for (const item of resolved) {
				const watch = this.#replica.watch(
					item.operation.artifact,
					item.variables,
					{ live: item.live }
				);
				watches.push(
					Object.freeze({
						watch,
						identity: item.identity,
						operation: item.operation.plan.operation,
						live: item.live
					})
				);
			}
		} catch (error) {
			for (const { watch } of watches) watch.destroy();
			throw error;
		}

		const retained: RetainedBoundary = {
			signature,
			boundary: validated.boundary,
			owners: 1,
			watches: Object.freeze(watches)
		};
		this.#instances.set(validated.id, retained);
		for (const item of watches) {
			const owners = (this.#identityOwners.get(item.identity) ?? 0) + 1;
			this.#identityOwners.set(item.identity, owners);
			this.#emit({
				action: 'acquire',
				boundary: retained.boundary,
				operation: item.operation,
				live: item.live,
				owners
			});
		}
		return this.#lease(validated.id, retained);
	}

	/** Close every old-scope owner while keeping the controller reusable. */
	disposeScope(): void {
		if (this.#destroyed) return;
		this.#disposeInstances(true);
	}

	destroy(): void {
		if (this.#destroyed) return;
		this.#destroyed = true;
		this.#disposeInstances(false);
	}

	#resolve<TSession, TProps>(
		instance: ReturnType<typeof validateInstance>,
		context: DistributedBoundaryVariableContext<TSession, TProps>
	): readonly ResolvedBoundary[] {
		const selected = this.#operations.filter(
			({ plan }) =>
				plan.kind === instance.kind && normalizeRoute(plan.route) === instance.route
		);
		if (selected.length === 0) {
			throw new Error(
				`Distributed SvelteKit boundary plan has no ${instance.boundary} selection`
			);
		}
		return Object.freeze(
			selected.map((operation) => {
				const variables = operation.binding.resolve(
					context as DistributedBoundaryVariableContext<
						unknown,
						Readonly<Record<string, unknown>>
					>
				);
				return Object.freeze({
					operation,
					variables,
					identity: operationIdentity(operation, variables),
					live: operation.artifact.live !== undefined
				});
			})
		);
	}

	#lease(instanceId: string, retained: RetainedBoundary): SveltekitBoundaryRetention {
		let released = false;
		return Object.freeze({
			release: (): void => {
				if (released) return;
				released = true;
				if (this.#instances.get(instanceId) !== retained) return;
				retained.owners -= 1;
				this.#emit({
					action: 'release',
					boundary: retained.boundary,
					owners: retained.owners
				});
				if (retained.owners > 0) return;
				this.#instances.delete(instanceId);
				this.#releaseWatches(retained);
			}
		});
	}

	#disposeInstances(scope: boolean): void {
		for (const [instanceId, retained] of [...this.#instances]) {
			this.#instances.delete(instanceId);
			if (scope) {
				this.#emit({
					action: 'scope-dispose',
					boundary: retained.boundary,
					owners: 0
				});
			}
			this.#releaseWatches(retained);
		}
	}

	#releaseWatches(retained: RetainedBoundary): void {
		for (const item of retained.watches) {
			item.watch.destroy();
			const owners = (this.#identityOwners.get(item.identity) ?? 1) - 1;
			if (owners > 0) {
				this.#identityOwners.set(item.identity, owners);
				continue;
			}
			this.#identityOwners.delete(item.identity);
			if (item.live) {
				this.#emit({
					action: 'final-unsubscribe',
					boundary: retained.boundary,
					operation: item.operation,
					live: true,
					owners: 0
				});
			}
		}
	}

	#emit(event: SveltekitBoundaryLifecycleDiagnostic): void {
		try {
			this.#diagnostic?.(Object.freeze(event));
		} catch {
			// Diagnostics are observational and cannot alter lifecycle ownership.
		}
	}
}

function validateInstance(instance: DistributedSvelteKitBoundaryInstance): Readonly<{
	id: string;
	route: string;
	kind: 'layout' | 'page';
	boundary: string;
}> {
	if (instance === null || typeof instance !== 'object') {
		throw new TypeError('Distributed SvelteKit boundary instance is required');
	}
	const id = typeof instance.id === 'string' ? instance.id.trim() : undefined;
	if (
		typeof id !== 'string' ||
		id.length === 0 ||
		new TextEncoder().encode(id).byteLength > MAX_INSTANCE_ID_BYTES
	) {
		throw new TypeError('Distributed SvelteKit boundary instance id is invalid');
	}
	if (instance.kind !== 'layout' && instance.kind !== 'page') {
		throw new TypeError('Distributed SvelteKit boundary instance kind is invalid');
	}
	const route = normalizeRoute(instance.route);
	return Object.freeze({
		id,
		route,
		kind: instance.kind,
		boundary: `${instance.kind}:${route}`
	});
}

function normalizeRoute(value: string): string {
	if (typeof value !== 'string' || !value.startsWith('/')) {
		throw new TypeError('Distributed SvelteKit boundary route must start with /');
	}
	const normalized = value.length === 1 ? value : value.replace(/\/+$/, '');
	return normalized.length === 0 ? '/' : normalized;
}

function operationIdentity(
	operation: DistributedBoundaryOperation,
	variables: GraphqlVariables
): string {
	const { protocol } = operation.artifact;
	return JSON.stringify([
		protocol.version,
		protocol.schemaHash,
		protocol.surface,
		operation.artifact.id,
		variables
	]);
}
