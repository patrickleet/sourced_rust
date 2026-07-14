/**
 * Browser GraphQL one-liners + re-exports (compat).
 * Prefer `import { useGraphql, defineResource } from '$lib/gql'`.
 */
import { requestGraphql } from './request.ts';
import type { GqlDocument } from './document.ts';
import type { GqlAuth, GqlResult } from './types.ts';

export type { GqlAuth, GqlResult } from './types.ts';
export { createGraphqlClient } from './create-client.ts';
export type { GraphqlClient, GraphqlClientOptions } from './create-client.ts';
export { defineResource } from './define-resource.ts';
export type { DefineResourceInput, GraphqlResource } from './define-resource.ts';
export { useGraphql } from './use-graphql.ts';
export { authFromPageData } from './auth-from-page.ts';
export type { PageGraphqlData } from './auth-from-page.ts';
export { documentToString } from './document.ts';
export type { GqlDocument } from './document.ts';
export { buildAuthHeaders } from './auth-headers.ts';

/** Browser POST /graphql (prefer useGraphql in pages). */
export async function browserGraphql<T = Record<string, unknown>>(
	document: GqlDocument,
	auth: GqlAuth = {},
	variables: Record<string, unknown> = {}
): Promise<GqlResult<T>> {
	return requestGraphql<T>('/graphql', document, auth, variables);
}
