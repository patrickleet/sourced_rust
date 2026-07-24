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
	fetch?: FetchLike;
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
	routes: readonly DistributedRouteOperation[];
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
	const routes = validateRoutes(options.routes);
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
			const auth =
				options.getAuth?.(pageData, event) ?? authFromPageData(pageData);
			const routeId = routeIdentity(event);
			const selected = routes.filter(({ plan }) => plan.route === routeId);
			if (selected.length === 0) {
				return {
					...pageData,
					gqlError: null
				};
			}

			const fetchImpl = event.fetch;
			const transport = createReplicaGraphqlTransport({
				getUrl: () => options.getUrl?.(event) ?? '/graphql',
				getAuth: () => auth,
				...(fetchImpl === undefined ? {} : { fetch: fetchImpl })
			});
			const replica = createDistributedReplica({ transport });
			const watches: Array<{
				operation: string;
				artifact: ReplicaOperationArtifact<unknown, GraphqlVariables>;
				variables: GraphqlVariables;
				watch: ReplicaWatch<unknown>;
			}> = [];
			try {
				for (const binding of selected) {
					let variables: GraphqlVariables;
					try {
						variables =
							(await options.variables?.[binding.plan.operation]?.(event)) ??
							{};
						const watch = replica.watch(
							binding.artifact,
							variables,
							{ live: false }
						);
						watches.push({
							operation: binding.plan.operation,
							artifact: binding.artifact,
							variables,
							watch
						});
					} catch (error) {
						if (options.variables?.[binding.plan.operation] === undefined) {
							throw new Error(
								`Distributed @load operation \`${binding.plan.operation}\` needs route variables; configure variables.${binding.plan.operation}(event) in createDistributedSvelteKitServer`,
								{ cause: error }
							);
						}
						throw error;
					}
				}
				await Promise.all(watches.map(({ watch }) => watch.refresh()));
				const errors = watches.flatMap(({ watch }) => watch.get().errors);
				for (const { artifact, variables, watch } of watches) {
					// Preserve exact rendered-operation reachability after the
					// temporary watch is released.
					replica.read(artifact, variables);
					watch.destroy();
				}
				const transfer =
					replica.scope === undefined
						? undefined
						: hydrationTransfer(
								replica.dehydrate(),
								selected.map(({ plan }) => plan.operation)
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
						errors[0]?.message ??
						(transfer === undefined
							? 'Distributed GraphQL response did not establish an authoritative cache scope'
							: null)
				};
			} finally {
				for (const { watch } of watches) watch.destroy();
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
	operations: readonly string[]
): Readonly<{
	hydration: SveltekitReplicaHydration;
	authority: SveltekitReplicaAuthority;
}> {
	return Object.freeze({
		hydration: Object.freeze({
			version: 1,
			state,
			operations: Object.freeze([...operations])
		}),
		authority: Object.freeze({
			version: 1,
			scope: state.scope
		})
	});
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
