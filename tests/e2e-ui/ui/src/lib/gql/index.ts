/**
 * Public GraphQL client surface for e2e-ui.
 * Prefer this barrel over deep imports when wiring routes.
 */
export type { GqlAuth, GqlResult } from './types.ts';
export type { GqlDocument } from './document.ts';
export { documentToString } from './document.ts';
export { buildAuthHeaders } from './auth-headers.ts';
export {
	wsConnectionInitPayload,
	applyWsDevHeaderParams
} from './auth-headers.ts';
export { requestGraphql } from './request.ts';
export { createGraphqlClient } from './create-client.ts';
export type { GraphqlClient, GraphqlClientOptions } from './create-client.ts';
export { defineResource } from './define-resource.ts';
export type { DefineResourceInput, GraphqlResource } from './define-resource.ts';
export { useGraphql, authFromPageData } from './use-graphql.ts';
export type { PageGraphqlData } from './auth-from-page.ts';
export { browserGraphql } from './client.ts';
