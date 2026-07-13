import type { PageLoad } from './$types';

/** Client-side load so static adapter build succeeds; points at WORKSHOP_BASE_URL. */
export const load: PageLoad = async ({ fetch }) => {
  const base =
    (typeof window !== 'undefined' && (window as unknown as { WORKSHOP_BASE_URL?: string }).WORKSHOP_BASE_URL) ||
    '';
  // Prefer relative /graphql (vite proxy) when running `vite dev`.
  const url = base ? `${base}/graphql` : '/graphql';
  try {
    const res = await fetch(url, {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
        'x-user-id': 'admin-1',
        'x-role': 'admin',
      },
      body: JSON.stringify({
        query: `{ products { product_id name price_cents owner_id listed } }`,
      }),
    });
    const data = await res.json();
    return {
      products: data?.data?.products ?? [],
      error: data?.errors?.[0]?.message ?? (res.ok ? null : `HTTP ${res.status}`),
      userId: 'admin-1',
      role: 'admin',
    };
  } catch (e) {
    return {
      products: [],
      error: e instanceof Error ? e.message : String(e),
      userId: 'admin-1',
      role: 'admin',
    };
  }
};
