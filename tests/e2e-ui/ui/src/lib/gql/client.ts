/**
 * Browser GraphQL — same-origin `/graphql` (Vite proxies to the API in dev).
 * Prefer createGraphqlClient when wiring app-wide; this keeps a one-liner for routes.
 */
import { requestGraphql } from './request';
import type { GqlAuth, GqlResult } from './types';

export type { GqlAuth, GqlResult } from './types';
export { createGraphqlClient } from './create-client';
export type { GraphqlClient, GraphqlClientOptions } from './create-client';
export { defineResource } from './define-resource';
export type { DefineResourceInput, GraphqlResource } from './define-resource';
export { useGraphql } from './use-graphql';
export { authFromPageData } from './auth-from-page';
export type { PageGraphqlData } from './auth-from-page';
// Note: Vite/SvelteKit resolves extensionless imports; node tests import .ts URLs.
// loadQuery lives in load-query.server.ts (server-only).

/**
 * Execute a GraphQL document from the browser.
 * Pass the same document strings as SSR (`$lib/gql/documents`).
 */
export async function browserGraphql<T = Record<string, unknown>>(
  document: string,
  auth: GqlAuth = {},
  variables: Record<string, unknown> = {}
): Promise<GqlResult<T>> {
  return requestGraphql<T>('/graphql', document, auth, variables);
}
