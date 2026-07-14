/**
 * Browser GraphQL client — same endpoint shape as SSR serverGraphql.
 * Uses same-origin `/graphql` (Vite proxies to the API in dev).
 */

export type GqlResult<T> = {
  data?: T;
  errors?: Array<{ message: string }>;
  status: number;
};

export type GqlAuth = {
  accessToken?: string | null;
  /** DevHeaders offline fallback only */
  userId?: string | null;
  role?: string | null;
};

/**
 * Execute a GraphQL document from the browser (or any env with fetch + relative URL).
 * Pass the same document strings as SSR (`$lib/gql/documents`).
 */
export async function browserGraphql<T = Record<string, unknown>>(
  document: string,
  auth: GqlAuth = {},
  variables: Record<string, unknown> = {}
): Promise<GqlResult<T>> {
  const headers: Record<string, string> = { 'content-type': 'application/json' };
  const token = auth.accessToken?.trim() || '';
  if (token) {
    headers.authorization = `Bearer ${token}`;
  } else if (auth.userId) {
    headers['x-user-id'] = auth.userId;
    headers['x-role'] = auth.role ?? 'user';
  }

  const res = await fetch('/graphql', {
    method: 'POST',
    headers,
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
        ? 'Bearer rejected — sign out and back in'
        : 'no access token — re-login');
    return { data: body.data, errors: [{ message: detail }], status: res.status };
  }

  return { data: body.data, errors: body.errors, status: res.status };
}
