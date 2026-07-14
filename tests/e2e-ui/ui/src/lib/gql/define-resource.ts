/**
 * Co-located GraphQL resource helper (DX pilot).
 * Holds one query document + mutation documents so SSR and browser share
 * the same string references (no dual string drift).
 */

export type ResourceMutations = Record<string, string>;

export type DefineResourceInput<
	TData = Record<string, unknown>,
	TMutations extends ResourceMutations = ResourceMutations
> = {
	/** GraphQL query document — same reference for SSR seed and client refetch */
	query: string;
	/** Named mutation documents (browser → POST /graphql) */
	mutations?: TMutations;
	/** Optional subscription document (same selection set as query when possible) */
	subscription?: string;
	/** Map raw query data → page props (used by load helpers) */
	select?: (data: TData) => unknown;
};

export type GraphqlResource<
	TData = Record<string, unknown>,
	TMutations extends ResourceMutations = ResourceMutations
> = {
	query: string;
	mutations: TMutations;
	subscription?: string;
	select?: (data: TData) => unknown;
};

/**
 * Define a co-located ops surface: one query + mutations object.
 * Pure — no SvelteKit env; wire URL/auth via loadQuery / useGraphql.
 */
export function defineResource<
	TData = Record<string, unknown>,
	TMutations extends ResourceMutations = ResourceMutations
>(def: DefineResourceInput<TData, TMutations>): GraphqlResource<TData, TMutations> {
	return {
		query: def.query,
		mutations: (def.mutations ?? ({} as TMutations)) as TMutations,
		subscription: def.subscription,
		select: def.select
	};
}
