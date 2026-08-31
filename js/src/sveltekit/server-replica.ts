import {
	createDistributedReplica,
	createReplicaGraphqlTransport,
	type ReplicaDehydratedState,
	type ReplicaOperationArtifact,
	type ReplicaWatch
} from '../replica/index.js';
import type { FetchLike } from '../request.js';
import type { GqlAuth, GraphqlVariables } from '../types.js';
import { authFromPageData, type PageGraphqlData } from './auth.js';
import {
	resolveDistributedBoundaryVariables,
	type DistributedBoundaryOperation,
	type DistributedBoundaryVariableContext
} from './boundary-variables.js';
import type {
	SveltekitDistributedPageData,
	SveltekitReplicaAuthority,
	SveltekitReplicaHydration
} from './replica.js';

export type DistributedRoutePlan = Readonly<{
	operation: string;
	route: string;
	source_path?: string;
	discovery: 'convention' | 'explicit';
}>;

export type DistributedRouteOperation = Readonly<{
	plan: DistributedRoutePlan;
	artifact: ReplicaOperationArtifact<unknown, GraphqlVariables>;
}>;

export type SveltekitServerLoadEventLike<TLocals = unknown> = Readonly<{
	locals: TLocals;
	route?: Readonly<{ id?: string | null }>;
	url?: URL;
	params?: Readonly<Record<string, string | undefined>>;
	parent?(): Promise<Readonly<Record<string, unknown>>>;
	request?: Readonly<{ signal: AbortSignal }>;
	fetch?: FetchLike;
	/**
	 * SvelteKit client-side navigation and hover preload (`__data.json`).
	 * Document SSR is `false`/`undefined`; those navigations must not wait on
	 * a fresh GraphQL replica — the browser replica already owns the cache.
	 */
	isDataRequest?: boolean;
}>;

export type DistributedRouteVariables<
	TEvent extends SveltekitServerLoadEventLike
> = Readonly<
	Record<
		string,
		(event: TEvent) => GraphqlVariables | Promise<GraphqlVariables>
	>
>;

export type CreateDistributedSvelteKitServerOptions<
	TSession extends NonNullable<PageGraphqlData['session']>,
	TEvent extends SveltekitServerLoadEventLike = SveltekitServerLoadEventLike
> = Readonly<{
	/** Transitional route inventory; removed after generated boundaries migrate. */
	routes?: readonly DistributedRouteOperation[];
	/** One executable binding per promoted page/layout island. */
	boundaries?: readonly DistributedBoundaryOperation<
		unknown,
		GraphqlVariables,
		TSession,
		Readonly<Record<string, unknown>>
	>[];
	getSession(event: TEvent): Promise<TSession | null>;
	getRole(
		session: TSession | null,
		event: TEvent
	): string | null | undefined;
	getAuth?(pageData: PageGraphqlData, event: TEvent): GqlAuth;
	/** Private API origin or same-origin `/graphql`; defaults to `/graphql`. */
	getUrl?(event: TEvent): string;
	/** Variables for routed operations that do not accept `{}`. */
	variables?: DistributedRouteVariables<TEvent>;
	/** Maximum simultaneous SSR island refreshes. Defaults to 8, maximum 32. */
	maxConcurrency?: number;
}>;

export type DistributedSvelteKitServer<TEvent> = Readonly<{
	load(event: TEvent): Promise<SveltekitDistributedPageData & {
		gqlError: string | null;
	}>;
}>;

/**
 * One app-level root layout loader for every compiler-discovered `@load`
 * operation. Each invocation creates a fresh replica and never shares SSR data
 * across requests.
 */
export function createDistributedSvelteKitServer<
	TSession extends NonNullable<PageGraphqlData['session']>,
	TEvent extends SveltekitServerLoadEventLike = SveltekitServerLoadEventLike
>(
	options: CreateDistributedSvelteKitServerOptions<TSession, TEvent>
): DistributedSvelteKitServer<TEvent> {
	const routes = validateRoutes(options.routes ?? []);
	const boundaries = validateBoundaryOperations(options.boundaries ?? []);
	const maxConcurrency = validateConcurrency(options.maxConcurrency ?? 8);
	return Object.freeze({
		async load(event: TEvent) {
			const session = await options.getSession(event);
			const accessToken = session?.accessToken ?? null;
			const engineRole = options.getRole(session, event) ?? null;
			const pageData: PageGraphqlData = {
				session,
				accessToken,
				engineRole
			};
			if (event.isDataRequest === true) {
				return {
					...pageData,
					gqlError: null
				};
			}
			const auth =
				options.getAuth?.(pageData, event) ?? authFromPageData(pageData);
			const routeId = routeIdentity(event);
			const selectedRoutes = routes.filter(({ plan }) => plan.route === routeId);
			const selectedBoundaries = boundaries
				.filter(({ plan }) =>
					plan.kind === 'page'
						? plan.route === routeId
						: layoutOwnsRoute(plan.route, routeId)
				)
				.sort(compareBoundaryOperations);
			const selected = [...selectedRoutes, ...selectedBoundaries];
			if (selected.length === 0) {
				return {
					...pageData,
					gqlError: null
				};
			}

			const requestSignal = event.request?.signal;
			const fetchImpl = event.fetch;
			const requestFetch: FetchLike | undefined =
				fetchImpl === undefined || requestSignal === undefined
					? fetchImpl
					: ((input, init) => {
							const transportSignal = init?.signal;
							const signal =
								transportSignal === undefined || transportSignal === null
									? requestSignal
									: AbortSignal.any([requestSignal, transportSignal]);
							return fetchImpl(input, { ...init, signal });
						});
			const transport = createReplicaGraphqlTransport({
				getUrl: () => options.getUrl?.(event) ?? '/graphql',
				getAuth: () => auth,
				...(requestFetch === undefined ? {} : { fetch: requestFetch })
			});
			const replica = createDistributedReplica({ transport });
			type Scheduled = Readonly<{
				identity: string;
				operation: string;
				artifact: ReplicaOperationArtifact<unknown, GraphqlVariables>;
				variables: GraphqlVariables;
			}>;
			type Execution = Scheduled & Readonly<{
				watch: ReplicaWatch<unknown>;
				failure: string | null;
			}>;
			const activeWatches = new Set<ReplicaWatch<unknown>>();
			const executions: Execution[] = [];
			let aborted = requestSignal?.aborted ?? false;
			const abort = (): void => {
				aborted = true;
				for (const watch of activeWatches) watch.destroy();
			};
			requestSignal?.addEventListener('abort', abort, { once: true });
			const boundaryContext: DistributedBoundaryVariableContext<
				TSession,
				Readonly<Record<string, unknown>>
			> = Object.freeze({
				params: event.params ?? Object.freeze({}),
				search: event.url?.searchParams ?? new URLSearchParams(),
				session,
				props:
					selectedBoundaries.length > 0 && event.parent !== undefined
						? await event.parent()
						: Object.freeze({})
			});
			try {
				const scheduled = new Map<string, Scheduled>();
				for (const binding of selected) {
					if (aborted) throw requestAborted();
					let variables: GraphqlVariables;
					try {
						variables = 'binding' in binding
							? resolveDistributedBoundaryVariables(
									binding.artifact,
									binding.binding.sources,
									boundaryContext
								)
							: (await options.variables?.[binding.plan.operation]?.(event)) ?? {};
						const identity = ssrOperationIdentity(binding.artifact, variables);
						if (scheduled.has(identity)) continue;
						scheduled.set(identity, {
							identity,
							operation: binding.plan.operation,
							artifact: binding.artifact,
							variables
						});
					} catch (error) {
						if (
							!('binding' in binding) &&
							options.variables?.[binding.plan.operation] === undefined
						) {
							throw new Error(
								`Distributed @load operation \`${binding.plan.operation}\` needs route variables; configure variables.${binding.plan.operation}(event) in createDistributedSvelteKitServer`,
								{ cause: error }
							);
						}
						throw error;
					}
				}
				const completed = await mapBounded(
					[...scheduled.values()],
					maxConcurrency,
					async (item): Promise<Execution> => {
						if (aborted) throw requestAborted();
						const watch = replica.watch(item.artifact, item.variables, { live: false });
						activeWatches.add(watch);
						let failure: string | null = null;
						try {
							await watch.refresh();
						} catch {
							failure = 'Distributed GraphQL island refresh failed';
						}
						if (watch.get().errors.length > 0) {
							failure = 'Distributed GraphQL island refresh failed';
						}
						return Object.freeze({ ...item, watch, failure });
					}
				);
				executions.push(...completed);
				if (aborted) throw requestAborted();
				const errors = executions.flatMap(({ failure }) =>
					failure === null ? [] : [failure]
				);
				for (const { artifact, variables, watch } of executions) {
					// Preserve exact rendered-operation reachability after the
					// temporary watch is released.
					replica.read(artifact, variables);
					watch.destroy();
					activeWatches.delete(watch);
				}
				const transfer =
					replica.scope === undefined
						? undefined
						: hydrationTransfer(
								replica.dehydrate(),
								selected.map(({ plan }) => plan.operation),
								selectedBoundaries.map(({ binding }) => binding.id)
							);
				return {
					...pageData,
					...(transfer === undefined
						? {}
						: {
								distributed: transfer.hydration,
								distributedAuthority: transfer.authority
							}),
					gqlError:
						errors[0] ??
						(transfer === undefined
							? 'Distributed GraphQL response did not establish an authoritative cache scope'
							: null)
				};
			} finally {
				requestSignal?.removeEventListener('abort', abort);
				for (const watch of activeWatches) watch.destroy();
				activeWatches.clear();
			}
		}
	});
}

/**
 * Explicit one-line fallback when the compiler cannot discover route ownership.
 *
 * Prefer co-locating `+page.graphql`; the compiler diagnostic includes the
 * equivalent `--route Operation=/route-id` registration.
 */
export function registerDistributedRoute<
	TData,
	TVariables extends GraphqlVariables
>(
	route: string,
	operation: string,
	artifact: ReplicaOperationArtifact<TData, TVariables>
): DistributedRouteOperation {
	return Object.freeze({
		plan: Object.freeze({
			operation: nonEmpty(operation, 'operation'),
			route: normalizeRoute(route),
			discovery: 'explicit' as const
		}),
		artifact: artifact as ReplicaOperationArtifact<unknown, GraphqlVariables>
	});
}

function hydrationTransfer(
	state: ReplicaDehydratedState,
	operations: readonly string[],
	bindings: readonly string[] = []
): Readonly<{
	hydration: SveltekitReplicaHydration;
	authority: SveltekitReplicaAuthority;
}> {
	return Object.freeze({
		hydration: Object.freeze({
			version: 1,
			state,
			operations: Object.freeze([...operations]),
			...(bindings.length === 0
				? {}
				: { bindings: Object.freeze([...new Set(bindings)].sort()) })
		}),
		authority: Object.freeze({
			version: 1,
			scope: state.scope
		})
	});
}

/**
 * Match a SvelteKit route id (`/blob/[[gameId]]`) to a browser pathname.
 */
export function matchDistributedRoute(
	routeId: string,
	pathname: string
): boolean {
	const route = normalizeRoute(routeId);
	const path = normalizePathname(pathname);
	if (route === path) return true;
	const routeParts = route
		.split('/')
		.filter(Boolean)
		.filter((part) => !(part.startsWith('(') && part.endsWith(')')));
	const pathParts = path.split('/').filter(Boolean);
	const failed = new Set<string>();
	const matches = (routeIndex: number, pathIndex: number): boolean => {
		const state = routeIndex + ':' + pathIndex;
		if (failed.has(state)) return false;
		if (routeIndex === routeParts.length) {
			return pathIndex === pathParts.length;
		}
		const part = routeParts[routeIndex]!;
		const optionalRest =
			part.startsWith('[[...') && part.endsWith(']]');
		const rest = part.startsWith('[...') && part.endsWith(']');
		if (optionalRest || rest) {
			for (let next = pathIndex; next <= pathParts.length; next += 1) {
				if (matches(routeIndex + 1, next)) return true;
			}
			failed.add(state);
			return false;
		}
		const optional =
			part.startsWith('[[') && part.endsWith(']]');
		if (optional) {
			if (matches(routeIndex + 1, pathIndex)) return true;
			if (
				pathIndex < pathParts.length &&
				matches(routeIndex + 1, pathIndex + 1)
			) {
				return true;
			}
			failed.add(state);
			return false;
		}
		const parameter = part.startsWith('[') && part.endsWith(']');
		if (
			(parameter && pathIndex < pathParts.length) ||
			(!parameter &&
				pathIndex < pathParts.length &&
				pathParts[pathIndex] === part)
		) {
			if (matches(routeIndex + 1, pathIndex + 1)) return true;
		}
		failed.add(state);
		return false;
	};
	return matches(0, 0);
}

function normalizePathname(pathname: string): string {
	if (typeof pathname !== 'string' || pathname.length === 0) return '/';
	const trimmed = pathname.replace(/\/+$/, '');
	return trimmed.length === 0 ? '/' : trimmed.startsWith('/') ? trimmed : `/${trimmed}`;
}

function validateRoutes(
	value: readonly DistributedRouteOperation[]
): readonly DistributedRouteOperation[] {
	if (!Array.isArray(value)) {
		throw new TypeError(
			'createDistributedSvelteKitServer requires generated DISTRIBUTED_ROUTE_OPERATIONS'
		);
	}
	const identities = new Set<string>();
	return Object.freeze(
		value.map((binding, index) => {
			if (
				binding === null ||
				typeof binding !== 'object' ||
				binding.plan === null ||
				typeof binding.plan !== 'object' ||
				binding.artifact === null ||
				typeof binding.artifact !== 'object'
			) {
				throw new TypeError(`invalid Distributed route binding at index ${index}`);
			}
			const operation = nonEmpty(binding.plan.operation, 'route operation');
			const route = normalizeRoute(binding.plan.route);
			if (binding.artifact.id.length === 0) {
				throw new TypeError(`invalid Distributed route artifact for ${operation}`);
			}
			const identity = `${route}\u0000${operation}`;
			if (identities.has(identity)) {
				throw new TypeError(
					`duplicate Distributed route operation ${operation} at ${route}`
				);
			}
			identities.add(identity);
			return Object.freeze({
				plan: Object.freeze({
					...binding.plan,
					operation,
					route
				}),
				artifact: binding.artifact
			});
		})
	);
}

function validateBoundaryOperations<TSession>(
	value: readonly DistributedBoundaryOperation<
		unknown,
		GraphqlVariables,
		TSession,
		Readonly<Record<string, unknown>>
	>[]
): readonly DistributedBoundaryOperation<
	unknown,
	GraphqlVariables,
	TSession,
	Readonly<Record<string, unknown>>
>[] {
	if (!Array.isArray(value)) {
		throw new TypeError('createDistributedSvelteKitServer boundaries must be an array');
	}
	const identities = new Set<string>();
	return Object.freeze(
		value.map((operation, index) => {
			if (
				operation === null ||
				typeof operation !== 'object' ||
				operation.binding?.version !== 1 ||
				operation.binding.artifactId !== operation.artifact?.id
			) {
				throw new TypeError(`invalid Distributed boundary operation at index ${index}`);
			}
			const route = normalizeRoute(operation.plan.route);
			const identity = `${operation.plan.kind}\u0000${route}\u0000${operation.plan.operation}`;
			if (identities.has(identity)) {
				throw new TypeError(
					`duplicate Distributed boundary operation ${operation.plan.operation} at ${route}`
				);
			}
			identities.add(identity);
			return Object.freeze({
				...operation,
				plan: Object.freeze({ ...operation.plan, route })
			});
		})
	);
}

function compareBoundaryOperations(
	left: DistributedBoundaryOperation,
	right: DistributedBoundaryOperation
): number {
	const leftDepth = left.plan.route.split('/').filter(Boolean).length;
	const rightDepth = right.plan.route.split('/').filter(Boolean).length;
	return (
		leftDepth - rightDepth ||
		left.plan.kind.localeCompare(right.plan.kind) ||
		left.plan.route.localeCompare(right.plan.route) ||
		left.plan.operation.localeCompare(right.plan.operation) ||
		left.binding.id.localeCompare(right.binding.id)
	);
}

function layoutOwnsRoute(layout: string, route: string): boolean {
	const owner = normalizeRoute(layout);
	const selected = normalizeRoute(route);
	return owner === '/' || selected === owner || selected.startsWith(`${owner}/`);
}

function ssrOperationIdentity(
	artifact: ReplicaOperationArtifact<unknown, GraphqlVariables>,
	variables: GraphqlVariables
): string {
	return JSON.stringify([
		artifact.protocol.version,
		artifact.protocol.schemaHash,
		artifact.protocol.surface,
		artifact.id,
		variables
	]);
}

function validateConcurrency(value: number): number {
	if (!Number.isSafeInteger(value) || value < 1 || value > 32) {
		throw new TypeError('Distributed SvelteKit maxConcurrency must be an integer from 1 through 32');
	}
	return value;
}

async function mapBounded<TInput, TResult>(
	values: readonly TInput[],
	concurrency: number,
	map: (value: TInput, index: number) => Promise<TResult>
): Promise<TResult[]> {
	const results = new Array<TResult>(values.length);
	let next = 0;
	const worker = async (): Promise<void> => {
		while (next < values.length) {
			const index = next;
			next += 1;
			results[index] = await map(values[index]!, index);
		}
	};
	await Promise.all(
		Array.from({ length: Math.min(concurrency, values.length) }, () => worker())
	);
	return results;
}

function requestAborted(): Error {
	const error = new Error('Distributed SvelteKit request was aborted');
	error.name = 'AbortError';
	return error;
}

function routeIdentity(event: SveltekitServerLoadEventLike): string {
	const route = event.route?.id;
	if (typeof route === 'string' && route.length > 0) return normalizeRoute(route);
	if (event.url !== undefined) return normalizeRoute(event.url.pathname);
	throw new Error(
		'Distributed SvelteKit route loading requires event.route.id or event.url.pathname'
	);
}

function normalizeRoute(value: string): string {
	const route = nonEmpty(value, 'route');
	if (!route.startsWith('/')) {
		throw new TypeError('Distributed route must start with /');
	}
	if (route.length === 1) return route;
	return route.replace(/\/+$/, '');
}

function nonEmpty(value: unknown, label: string): string {
	if (typeof value !== 'string' || value.trim().length === 0) {
		throw new TypeError(`Distributed ${label} must be a non-empty string`);
	}
	return value.trim();
}
