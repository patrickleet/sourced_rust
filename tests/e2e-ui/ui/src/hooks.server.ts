import { redirect, type Handle } from '@sveltejs/kit';
import { sequence } from '@sveltejs/kit/hooks';
import { handle as authHandle } from './auth';

const PROTECTED = ['/todos', '/chat', '/session'];

async function authorizationHandle({ event, resolve }: Parameters<Handle>[0]) {
  const path = event.url.pathname;
  if (PROTECTED.some((p) => path === p || path.startsWith(`${p}/`))) {
    const session = await event.locals.auth();
    if (!session?.user) {
      const callbackUrl = encodeURIComponent(event.url.pathname + event.url.search);
      throw redirect(303, `/signin?callbackUrl=${callbackUrl}`);
    }
  }
  return resolve(event);
}

export const handle: Handle = sequence(authHandle, authorizationHandle);
