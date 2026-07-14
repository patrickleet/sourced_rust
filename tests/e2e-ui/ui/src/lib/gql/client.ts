/**
 * Browser GraphQL — same-origin `/graphql` (Vite proxies to the API in dev).
 * Prefer createGraphqlClient when wiring app-wide; this keeps a one-liner for routes.
 */
import { requestGraphql } from './request.ts';
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
// Note: Vite/SvelteKit resolves extensionless imports; node tests import .ts URLs.
// loadQuery lives in load-query.server.ts (server-only).

/**
 * Execute a GraphQL document from the browser.
 * Prefer co-located `*.gql` generated documents via useGraphql / resources.
 */
export async function browserGraphql<T = Record<string, unknown>>(
  document: import('./document').GqlDocument,
  auth: GqlAuth = {},
  variables: Record<string, unknown> = {}
): Promise<GqlResult<T>> {
  return requestGraphql<T>('/graphql', document, auth, variables);
}
