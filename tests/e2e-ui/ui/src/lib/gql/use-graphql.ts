/**
 * Browser binder: page data → GqlAuth + createGraphqlClient (POST /graphql).
 * Keep getAuth lazy so accessToken stays current after session updates.
 */
import { createGraphqlClient, type GraphqlClient } from './create-client.ts';
import { authFromPageData, type PageGraphqlData } from './auth-from-page.ts';

export type { PageGraphqlData } from './auth-from-page.ts';
export { authFromPageData } from './auth-from-page.ts';

/**
 * Client bound to same-origin `/graphql` (Vite proxies to the API in dev).
 * Pass a getter so reactive page data is read on each request.
 */
export function useGraphql(getData: () => PageGraphqlData): GraphqlClient {
	return createGraphqlClient({
		getUrl: () => '/graphql',
		getAuth: () => authFromPageData(getData())
	});
}
