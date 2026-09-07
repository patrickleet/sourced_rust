import { json } from '@sveltejs/kit';
import { distributed } from '$lib/server/distributed';
import { isCurrentSession } from '$lib/server/require-auth';
import type { RequestHandler } from './$types';

export const POST: RequestHandler = async (event) => {
	const { locals } = event;
	const session = await locals.auth();

	if (!isCurrentSession(session)) {
		return json({ authenticated: false, error: session?.error }, { status: 401 });
	}

	// A refresh may change the bearer credential without changing effective
	// permissions. Supply a new, independently authorized route seed so the
	// browser can prove same-scope reuse before fencing the old credential.
	// Route selection is input, never authority: the generated user surface
	// and GraphQL executor still authorize every selected operation.
	let pageData;
	if (event.request.headers.get('content-type')?.includes('application/json')) {
		let route;
		try {
			route = await event.request.json();
		} catch {
			return json({ error: 'Invalid route' }, { status: 400 });
		}
		if (
			typeof route?.id !== 'string' ||
			typeof route?.path !== 'string' ||
			!route.path.startsWith('/') ||
			route.path.startsWith('//') ||
			route.params === null ||
			typeof route.params !== 'object' ||
			Array.isArray(route.params) ||
			Object.values(route.params).some(value => typeof value !== 'string')
		) {
			return json({ error: 'Invalid route' }, { status: 400 });
		}
		const url = new URL(route.path, event.url.origin);
		if (url.origin !== event.url.origin) {
			return json({ error: 'Invalid route' }, { status: 400 });
		}
		pageData = await distributed.load({
			...event,
			locals: { ...locals, auth: async () => session },
			isDataRequest: false,
			url,
			route: { id: route.id },
			params: route.params
		});
	}

	return json({
		pageData,
		authenticated: true,
		expires: session.expires,
		expiresAt: session.expiresAt,
		refreshAfter: session.refreshAfter,
		hasAccessToken: session.hasAccessToken,
		hasRefreshToken: session.hasRefreshToken,
		hasIdToken: session.hasIdToken,
		error: session.error
	}, { headers: { 'cache-control': 'no-store' } });
};
