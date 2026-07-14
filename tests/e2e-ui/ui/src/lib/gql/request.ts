/**
 * Single HTTP GraphQL request path for browser and SSR.
 * URL and auth are injected by callers (see createGraphqlClient / wrappers).
 */
import type { GqlAuth, GqlResult } from './types';

export function buildAuthHeaders(auth: GqlAuth = {}): Record<string, string> {
  const headers: Record<string, string> = { 'content-type': 'application/json' };
  const token = auth.accessToken?.trim() || '';
  if (token) {
    headers.authorization = `Bearer ${token}`;
  } else if (auth.userId) {
    headers['x-user-id'] = auth.userId;
    headers['x-role'] = auth.role ?? 'user';
  }
  return headers;
}

/**
 * POST GraphQL document to `url`. Real entry point used by browser + SSR wrappers.
 */
export async function requestGraphql<T = Record<string, unknown>>(
  url: string,
  document: string,
  auth: GqlAuth = {},
  variables: Record<string, unknown> = {}
): Promise<GqlResult<T>> {
  const token = auth.accessToken?.trim() || '';
  const res = await fetch(url, {
    method: 'POST',
    headers: buildAuthHeaders(auth),
    body: JSON.stringify({ query: document, variables })
  });

  const body = (await res.json().catch(() => ({}))) as {
    data?: T;
    errors?: Array<{ message: string }>;
    error?: string;
  };

  if (res.status === 401) {
    const detail =
      body.errors?.[0]?.message ||
      body.error ||
      (token
        ? 'Bearer rejected (audience/issuer/expiry) — sign out and back in'
        : 'no access token — re-login; check OIDC_SCOPES includes project aud');
    return {
      data: body.data,
      errors: [{ message: detail }],
      status: res.status
    };
  }

  return { data: body.data, errors: body.errors, status: res.status };
}
