/**
 * Server-side GraphQL (SSR) — same request path as browser (`requestGraphql`).
 * Hits the API origin directly (not the Vite proxy).
 */
import { env } from '$env/dynamic/private';
import { env as publicEnv } from '$env/dynamic/public';
import { requestGraphql, type GqlDocument } from '$lib/gql/request';
import type { GqlAuth, GqlResult } from '$lib/gql/types';
import { createGraphqlClient } from '$lib/gql/create-client';

export type { GqlResult } from '$lib/gql/types';

/** Peel accidental outer quotes from env (Make-include / double-wrap pollution). */
function cleanEnv(v: string | undefined): string {
  let s = (v ?? '').trim();
  for (let i = 0; i < 2; i++) {
    if (
      (s.startsWith("'") && s.endsWith("'") && s.length >= 2) ||
      (s.startsWith('"') && s.endsWith('"') && s.length >= 2)
    ) {
      s = s.slice(1, -1).trim();
    } else {
      break;
    }
  }
  return s;
}

export function apiBase(): string {
  return (
    cleanEnv(env.E2E_API_ORIGIN) ||
    cleanEnv(env.E2E_BASE_URL) ||
    cleanEnv(publicEnv.PUBLIC_E2E_API_ORIGIN) ||
    'http://127.0.0.1:8791'
  );
}

export function graphqlHttpUrl(): string {
  return `${apiBase()}/graphql`;
}

/**
 * SSR GraphQL — same documents as co-located `.gql` / resources + browser client.
 */
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
