/**
 * Co-located GraphQL resource helper.
 * Holds one query document + mutations (+ optional subscription) so SSR and
 * browser share the same document references. Prefer TypedDocumentNode from
 * co-located `*.gql` codegen (`*.generated.ts`).
 */
import type { GqlDocument } from './document.ts';

export type ResourceMutations = Record<string, GqlDocument>;

export type DefineResourceInput<
	TData = Record<string, unknown>,
	TMutations extends ResourceMutations = ResourceMutations
> = {
	/** Query document — same reference for SSR seed and client refetch */
	query: GqlDocument;
	/** Named mutation documents (browser → POST /graphql) */
	mutations?: TMutations;
	/** Optional subscription document (same selection set as query when possible) */
	subscription?: GqlDocument;
	/** Map raw query data → page props (used by load helpers) */
	select?: (data: TData) => unknown;
};

export type GraphqlResource<
	TData = Record<string, unknown>,
	TMutations extends ResourceMutations = ResourceMutations
> = {
	query: GqlDocument;
	mutations: TMutations;
	subscription?: GqlDocument;
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
