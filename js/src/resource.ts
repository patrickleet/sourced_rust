/** Co-locate a query and optional subscription without framework coupling. */
import type { GqlDocument } from './document.js';
import type { GraphqlVariables } from './types.js';

export type DefineResourceInput<
	TData = Record<string, unknown>,
	TVariables extends GraphqlVariables = GraphqlVariables,
	TSelected = unknown
> = {
	/** Query document shared by server loads and browser refetches. */
	query: GqlDocument<TData, TVariables>;
	/** Optional live document, ideally with the same result shape as the query. */
	subscription?: GqlDocument<TData, TVariables>;
	/** Map raw GraphQL data to an application-facing value. */
	select?: (data: TData) => TSelected;
};

export type GraphqlResource<
	TData = Record<string, unknown>,
	TVariables extends GraphqlVariables = GraphqlVariables,
	TSelected = unknown
> = DefineResourceInput<TData, TVariables, TSelected>;

/** Preserve typed document identity while declaring a query/subscription resource. */
export function defineResource<
	TData = Record<string, unknown>,
	TVariables extends GraphqlVariables = GraphqlVariables,
	TSelected = unknown
>(
	definition: DefineResourceInput<TData, TVariables, TSelected>
): GraphqlResource<TData, TVariables, TSelected> {
	return definition;
}
