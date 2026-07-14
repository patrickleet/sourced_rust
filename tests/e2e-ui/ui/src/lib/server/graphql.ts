/**
 * Server-side GraphQL (SSR) — same request path as browser (`requestGraphql`).
 * Hits the API origin directly (not the Vite proxy).
 */
import { env } from '$env/dynamic/private';
import { env as publicEnv } from '$env/dynamic/public';
import { cleanEnvValue } from '$lib/clean-env';
import { createGraphqlClient } from '$lib/gql/create-client';
import { requestGraphql, type GqlDocument } from '$lib/gql/request';
import type { GqlAuth, GqlResult } from '$lib/gql/types';

export type { GqlResult } from '$lib/gql/types';

export function apiBase(): string {
	return (
		cleanEnvValue(env.E2E_API_ORIGIN) ||
		cleanEnvValue(env.E2E_BASE_URL) ||
		cleanEnvValue(publicEnv.PUBLIC_E2E_API_ORIGIN) ||
		'http://127.0.0.1:8791'
	);
}

export function graphqlHttpUrl(): string {
	return `${apiBase()}/graphql`;
}

/** SSR GraphQL — same documents as co-located `.gql` / resources + browser client. */
export async function serverGraphql<T = Record<string, unknown>>(
	document: GqlDocument,
	opts: GqlAuth & { variables?: Record<string, unknown> } = {}
): Promise<GqlResult<T>> {
	const { variables, ...auth } = opts;
	return requestGraphql<T>(graphqlHttpUrl(), document, auth, variables ?? {});
}

/** Factory wired to API origin — use in load functions when preferred over serverGraphql. */
export function createServerGraphqlClient(getAuth: () => GqlAuth | Promise<GqlAuth>) {
	return createGraphqlClient({
		getUrl: graphqlHttpUrl,
		getAuth
	});
}
