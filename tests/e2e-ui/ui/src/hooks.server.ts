import { redirect, type Handle, type RequestEvent } from '@sveltejs/kit';
import { handle as authHandle } from './auth';
import { sequence } from '@sveltejs/kit/hooks';

async function authorizationHandle({ event, resolve }: { event: RequestEvent; resolve: (event: RequestEvent) => Response | Promise<Response>; }) {
  // Protect admin (website) + fixture app routes
  const path = event.url.pathname;
  // /chat is intentionally public (anonymous GraphQL read on e2e-ui-public).
  const protectedPrefix =
    path.startsWith('/admin') ||
    path === '/todos' ||
    path.startsWith('/todos/') ||
    path === '/blob' ||
    path.startsWith('/blob/') ||
    path === '/session' ||
    path.startsWith('/session/');

  if (protectedPrefix) {
    const session = await event.locals.auth();
    if (!session?.user) {
      const callbackUrl = encodeURIComponent(event.url.pathname + event.url.search);
      // Custom Login V2 pages at /login (not Zitadel-hosted UI)
      throw redirect(303, `/login?callbackUrl=${callbackUrl}`);
    }
  }

  return resolve(event);
}

export const handle: Handle = sequence(authHandle, authorizationHandle);
