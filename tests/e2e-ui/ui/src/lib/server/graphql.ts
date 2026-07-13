/**
 * Server-side GraphQL (SSR) — always prefer this in +page.server.ts loads.
 * Uses the Auth.js access token (OidcBearer) against the Distributed API.
 */
import { env } from '$env/dynamic/private';
import { env as publicEnv } from '$env/dynamic/public';

export function apiBase(): string {
  return (
    env.E2E_API_ORIGIN?.trim() ||
    env.E2E_BASE_URL?.trim() ||
    publicEnv.PUBLIC_E2E_API_ORIGIN?.trim() ||
    'http://127.0.0.1:8791'
  );
}

export type GqlResult<T> = {
  data?: T;
  errors?: Array<{ message: string }>;
  status: number;
};

export async function serverGraphql<T = Record<string, unknown>>(
  query: string,
  opts: {
    accessToken?: string | null;
    /** DevHeaders fallback when OIDC is not configured (local offline). */
    userId?: string;
    role?: string;
    variables?: Record<string, unknown>;
  } = {}
): Promise<GqlResult<T>> {
  const headers: Record<string, string> = { 'content-type': 'application/json' };
  if (opts.accessToken) {
    headers.authorization = `Bearer ${opts.accessToken}`;
  } else if (opts.userId) {
    headers['x-user-id'] = opts.userId;
    headers['x-role'] = opts.role ?? 'user';
  }

  const res = await fetch(`${apiBase()}/graphql`, {
    method: 'POST',
    headers,
    body: JSON.stringify({ query, variables: opts.variables ?? {} })
  });
  const body = (await res.json().catch(() => ({}))) as {
    data?: T;
    errors?: Array<{ message: string }>;
  };
  return { data: body.data, errors: body.errors, status: res.status };
}

export async function serverCommand(
  command: string,
  body: Record<string, unknown>,
  opts: { accessToken?: string | null; userId?: string; role?: string }
) {
  const headers: Record<string, string> = { 'content-type': 'application/json' };
  if (opts.accessToken) {
    headers.authorization = `Bearer ${opts.accessToken}`;
  } else if (opts.userId) {
    headers['x-user-id'] = opts.userId;
    headers['x-role'] = opts.role ?? 'user';
  }
  const res = await fetch(`${apiBase()}/${command}`, {
    method: 'POST',
    headers,
    body: JSON.stringify(body)
  });
  const json = await res.json().catch(() => ({}));
  return { ok: res.ok, status: res.status, body: json };
}
