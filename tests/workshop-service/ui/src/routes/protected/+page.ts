import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch }) => {
  const url = '/graphql';
  try {
    const res = await fetch(url, {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
        'x-user-id': 'customer-1',
        'x-role': 'customer',
      },
      body: JSON.stringify({
        query: `{ workshop_orders { order_id product_id customer_id quantity status } }`,
      }),
    });
    const data = await res.json();
    return {
      orders: data?.data?.workshop_orders ?? [],
      error: data?.errors?.[0]?.message ?? (res.ok ? null : `HTTP ${res.status}`),
      userId: 'customer-1',
      role: 'customer',
    };
  } catch (e) {
    return {
      orders: [],
      error: e instanceof Error ? e.message : String(e),
      userId: 'customer-1',
      role: 'customer',
    };
  }
};
