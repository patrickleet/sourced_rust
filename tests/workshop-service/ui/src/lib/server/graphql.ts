/**
 * Server-side GraphQL client (pattern from the-website control-plane GraphQL helper).
 * Forwards identity headers so OidcBearer/DevHeaders sessions work against the fixture.
 */

const base = () => process.env.WORKSHOP_BASE_URL ?? 'http://127.0.0.1:8791';

export async function workshopGraphql(
  query: string,
  opts: { userId: string; role: string; variables?: Record<string, unknown> }
) {
  const res = await fetch(`${base()}/graphql`, {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      'x-user-id': opts.userId,
      'x-role': opts.role,
    },
    body: JSON.stringify({ query, variables: opts.variables ?? {} }),
  });
  if (!res.ok) {
    throw new Error(`GraphQL HTTP ${res.status}: ${await res.text()}`);
  }
  return res.json();
}
