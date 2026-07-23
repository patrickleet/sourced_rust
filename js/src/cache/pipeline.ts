/** Optimistic -> network -> result -> effects -> reconcile command pipeline. */
import type { GqlError, GraphqlVariables } from '../types.js';
import type { GraphqlResponseExtensions } from '../protocol.js';
import {
	applyCacheOps,
	applyProjectionPayload,
	fx,
	rollback,
	writeServerDataPreservingPending,
	type CacheOp,
	type CacheTarget,
	type CommandPolicy,
	type Effect,
	type ReconcileKind,
	type ResultKind
} from './ops.js';
import { cacheKey, type QueryCache } from './query-cache.js';

export type NetworkResult<TData = Record<string, unknown>> = {
	data?: TData | null;
	errors?: GqlError[] | null;
	/** Validated GraphQL response extensions preserved for command receipts. */
	extensions?: GraphqlResponseExtensions;
	/** HTTP status when the transport has one. Network exceptions use 0. */
	status?: number;
};

export type CommandPipelineOptions = {
	policy?: CommandPolicy;
	optimistic?: CommandPolicy['optimistic'];
	result?: CommandPolicy['result'];
	reconcile?: CommandPolicy['reconcile'];
	onSuccess?: (context: { data: unknown }) => Array<CacheOp | Effect>;
	onError?: (context: { errors: GqlError[] }) => Array<CacheOp | Effect>;
	onSettled?: () => void;
	/** Set false during SSR to skip optimistic and cache mutations. */
	browser?: boolean;
};

export type PipelineDeps = {
	cache: QueryCache;
	request: (
		document: string,
		variables?: GraphqlVariables
	) => Promise<NetworkResult>;
	refetch?: (
		document: string,
		variables?: GraphqlVariables
	) => Promise<NetworkResult>;
	/** Set only after a generated causal receipt was strictly correlated. */
	retainOptimismOnError?: (result: NetworkResult<unknown>) => boolean;
	runEffects?: (effects: Effect[]) => void;
};

type MergedPolicy = {
	resultKind: ResultKind;
	applyTargets?: CacheTarget[];
	reconcileKind: ReconcileKind;
	reconcileDocument?: string;
	reconcileVariables?: GraphqlVariables;
	optimistic?: CommandPolicy['optimistic'];
};

function mergePolicy(
	defaults: CommandPolicy | undefined,
	options: CommandPipelineOptions
): MergedPolicy {
	const result = options.result ?? defaults?.result;
	const reconcile = options.reconcile ?? defaults?.reconcile;
	return {
		resultKind: result?.kind ?? 'ack',
		applyTargets: result?.apply?.targets,
		reconcileKind: reconcile?.kind ?? 'none',
		reconcileDocument: reconcile?.document,
		reconcileVariables: reconcile?.variables,
		optimistic: options.optimistic ?? defaults?.optimistic
	};
}

function splitOps(items: Array<CacheOp | Effect>): { ops: CacheOp[]; effects: Effect[] } {
	const ops: CacheOp[] = [];
	const effects: Effect[] = [];
	for (const item of items) {
		if ('op' in item) ops.push(item);
		else effects.push(item);
	}
	return { ops, effects };
}

/** Execute one generated command through the configured cache pipeline. */
export async function runCommandPipeline<TData = Record<string, unknown>>(
	deps: PipelineDeps,
	commandDocument: string,
	input: unknown,
	options: CommandPipelineOptions = {}
): Promise<NetworkResult<TData>> {
	const browser = options.browser !== false;
	const policy = mergePolicy(options.policy, options);
	const cacheGeneration = deps.cache.generation;
	const cacheIsCurrent = () => deps.cache.generation === cacheGeneration;
	let snapshots: ReturnType<typeof applyCacheOps> = [];

	if (browser && policy.optimistic?.targets.length && policy.optimistic.row) {
		snapshots = applyCacheOps(
			deps.cache,
			policy.optimistic.targets.map((target) => ({
				op: 'upsert',
				target,
				row: { ...policy.optimistic!.row! }
			}))
		);
	}

	let result: NetworkResult<TData>;
	try {
		result = (await deps.request(
			commandDocument,
			input === undefined ? undefined : { input }
		)) as NetworkResult<TData>;
	} catch (error) {
		const errors: GqlError[] = [
			{ message: error instanceof Error ? error.message : String(error) }
		];
		if (!cacheIsCurrent()) {
			options.onSettled?.();
			return { data: null, errors, status: 0 };
		}
		if (browser && snapshots.length) rollback(deps.cache, snapshots);
		const { ops, effects } = splitOps(options.onError?.({ errors }) ?? []);
		if (browser && ops.length) applyCacheOps(deps.cache, ops);
		deps.runEffects?.(effects);
		options.onSettled?.();
		return { data: null, errors, status: 0 };
	}

	if (!cacheIsCurrent()) {
		options.onSettled?.();
		return result;
	}

	if (result.errors?.length) {
		const correlatedPending =
			deps.retainOptimismOnError?.(result) === true;
		if (
			isAmbiguousCausalTransport(result) ||
			correlatedPending
		) {
			// The server may have committed. Retain the named optimistic layer;
			// only an authoritative replay/status outcome may confirm or reject it.
			if (
				correlatedPending &&
				browser &&
				policy.optimistic?.targets.length
			) {
				markOptimisticTargetsPending(
					deps.cache,
					policy.optimistic.targets
				);
			}
			options.onSettled?.();
			return result;
		}
		if (browser && snapshots.length) rollback(deps.cache, snapshots);
		const { ops, effects } = splitOps(
			options.onError?.({ errors: result.errors }) ?? [
				fx.alert(result.errors[0]?.message ?? 'Failed')
			]
		);
		if (browser && ops.length) applyCacheOps(deps.cache, ops);
		deps.runEffects?.(effects);
		options.onSettled?.();
		return result;
	}

	if (browser) {
		if (policy.resultKind === 'projection') {
			const payload = projectionPayload(result.data);
			if (payload && policy.applyTargets?.length) {
				applyProjectionPayload(deps.cache, policy.applyTargets, payload);
			}
		} else if (
			(policy.resultKind === 'ack' || policy.resultKind === 'fact') &&
			policy.optimistic?.targets.length
		) {
			markOptimisticTargetsPending(
				deps.cache,
				policy.optimistic.targets
			);
		}
	}

	const { ops, effects } = splitOps(options.onSuccess?.({ data: result.data }) ?? []);
	if (browser && ops.length) applyCacheOps(deps.cache, ops);
	deps.runEffects?.(effects);

	if (
		browser &&
		cacheIsCurrent() &&
		policy.reconcileKind === 'refetch' &&
		policy.reconcileDocument &&
		deps.refetch
	) {
		const refetched = await deps.refetch(
			policy.reconcileDocument,
			policy.reconcileVariables
		);
		if (
			cacheIsCurrent() &&
			refetched.data !== undefined &&
			refetched.data !== null &&
			!refetched.errors?.length
		) {
			const reconcileList = (options.reconcile ?? options.policy?.reconcile)?.list;
			writeServerDataPreservingPending(
				deps.cache,
				policy.reconcileDocument,
				policy.reconcileVariables,
				refetched.data,
				reconcileList ? { list: reconcileList } : undefined
			);
		}
	} else if (
		browser &&
		cacheIsCurrent() &&
		policy.reconcileKind === 'invalidate' &&
		policy.reconcileDocument
	) {
		deps.cache.invalidate(
			cacheKey(policy.reconcileDocument, policy.reconcileVariables)
		);
	}

	options.onSettled?.();
	return result;
}

function isAmbiguousCausalTransport(result: NetworkResult<unknown>): boolean {
	return (
		result.status === 0 &&
		result.errors?.some(
			(error) =>
				error.extensions?.code === 'CAUSAL_TRANSPORT_AMBIGUOUS'
		) === true
	);
}

function markOptimisticTargetsPending(
	cache: QueryCache,
	targets: readonly CacheTarget[]
): void {
	for (const target of targets) {
		const key = cacheKey(target.document, target.variables);
		const entry = cache.get(key);
		if (entry) {
			cache.set(key, {
				...entry,
				pending: true,
				optimistic: true
			});
		}
	}
}

function projectionPayload(data: unknown): Record<string, unknown> | null {
	if (data === null || typeof data !== 'object' || Array.isArray(data)) return null;
	const record = data as Record<string, unknown>;
	const first = Object.values(record)[0];
	return first !== null && typeof first === 'object' && !Array.isArray(first)
		? (first as Record<string, unknown>)
		: record;
}

export { fx };
export type {
	CacheOp,
	CommandPolicy,
	Effect,
	ReconcileKind,
	ResultKind
};
