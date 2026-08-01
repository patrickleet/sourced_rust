import type { Handle, RequestEvent } from '@sveltejs/kit';
import { handle as authHandle } from './auth';
import { sequence } from '@sveltejs/kit/hooks';
import { requireAuth } from '$lib/server/require-auth';

/** Paths that require a session (chat is public; home is public). */
function isProtectedPath(path: string): boolean {
	return (
		path.startsWith('/admin') ||
		path === '/todos' ||
		path.startsWith('/todos/') ||
		path === '/blob' ||
		path.startsWith('/blob/') ||
		path === '/session' ||
		path.startsWith('/session/')
	);
}

async function authorizationHandle({
	event,
	resolve
}: {
	event: RequestEvent;
	resolve: (event: RequestEvent) => Response | Promise<Response>;
}) {
	const path = event.url.pathname;
	// Full document loads always hit this. Client nav also needs +page.server
	// requireAuth (root layout is cached and may skip a round-trip).
	if (isProtectedPath(path)) {
		await requireAuth(event);
	}

	return resolve(event);
}

export const handle: Handle = sequence(authHandle, authorizationHandle);
