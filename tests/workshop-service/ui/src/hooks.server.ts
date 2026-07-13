import type { Handle } from '@sveltejs/kit';

/**
 * Session pattern inspired by hops `sites/the-website` (auth headers for GraphQL).
 * Dev: cookies `x-user-id` / `x-role` (or env defaults). Production: replace with Auth.js OIDC session.
 */
export const handle: Handle = async ({ event, resolve }) => {
  event.locals.userId =
    event.cookies.get('x-user-id') ?? process.env.WORKSHOP_UI_USER_ID ?? 'customer-1';
  event.locals.role =
    event.cookies.get('x-role') ?? process.env.WORKSHOP_UI_ROLE ?? 'customer';
  return resolve(event);
};
