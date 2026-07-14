/**
 * Server-side GraphQL (SSR) — always prefer this in +page.server.ts loads.
 * Uses the Auth.js access token (OidcBearer) against the Distributed API.
 *
 * Under OidcBearer the API rejects DevHeaders (`x-user-id`). Always send Bearer
 * when the session has an access token; never fall back to headers in that case.
 */
import { env } from '$env/dynamic/private';
import { env as publicEnv } from '$env/dynamic/public';

/** Peel accidental outer quotes from env (Make-include / double-wrap pollution). */
function cleanEnv(v: string | undefined): string {
  let s = (v ?? '').trim();
  if (
    (s.startsWith("'") && s.endsWith("'") && s.length >= 2) ||
    (s.startsWith('"') && s.endsWith('"') && s.length >= 2)
  ) {
    s = s.slice(1, -1).trim();
  }
  // Collapse accidental ''http://...'' double wraps
  if (
    (s.startsWith("'") && s.endsWith("'") && s.length >= 2) ||
    (s.startsWith('"') && s.endsWith('"') && s.length >= 2)
  ) {
    s = s.slice(1, -1).trim();
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

export type GqlResult<T> = {
  data?: T;
  errors?: Array<{ message: string }>;
  status: number;
};

/**
 * SSR GraphQL — same documents as `$lib/gql/documents` + browserGraphql.
 * Hits the API origin directly (not the Vite proxy).
 */
export async function serverGraphql<T = Record<string, unknown>>(
  /** Same document string the browser uses */
  document: string,
  opts: {
    accessToken?: string | null;
    /** DevHeaders fallback when OIDC is not configured (local offline only). */
    userId?: string;
    role?: string;
    variables?: Record<string, unknown>;
  } = {}
): Promise<GqlResult<T>> {
  const headers: Record<string, string> = { 'content-type': 'application/json' };
  const token = opts.accessToken?.trim() || '';
  if (token) {
    headers.authorization = `Bearer ${token}`;
  } else if (opts.userId) {
    // Offline DevHeaders path only — OidcBearer strips these and returns 401.
    headers['x-user-id'] = opts.userId;
    headers['x-role'] = opts.role ?? 'user';
  }

  const url = `${apiBase()}/graphql`;
  const res = await fetch(url, {
    method: 'POST',
    headers,
    body: JSON.stringify({ query: document, variables: opts.variables ?? {} })
  });
  const body = (await res.json().catch(() => ({}))) as {
    data?: T;
    errors?: Array<{ message: string }>;
    error?: string;
  };

  // Surface OIDC gate failures with enough context to debug missing Bearer vs bad token.
  if (res.status === 401) {
    const detail =
      body.errors?.[0]?.message ||
      body.error ||
      (token
        ? 'Bearer rejected (audience/issuer/expiry) — sign out and back in after make up'
        : 'no access token on session — re-login; check OIDC_SCOPES includes project aud');
    return {
      data: body.data,
      errors: [{ message: detail }],
      status: res.status
    };
  }

  return { data: body.data, errors: body.errors, status: res.status };
}
