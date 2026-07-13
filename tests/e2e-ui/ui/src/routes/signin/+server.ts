import { error } from '@sveltejs/kit';
import type { RequestHandler } from './$types';

function safeCallbackUrl(url: URL) {
	const callbackUrl = url.searchParams.get('callbackUrl');
	if (!callbackUrl) return url.origin;

	try {
		const parsed = new URL(callbackUrl, url.origin);
		if (parsed.origin !== url.origin) return url.origin;
		return parsed.toString();
	} catch {
		return url.origin;
	}
}

export const GET: RequestHandler = async (event) => {
	const callbackUrl = safeCallbackUrl(event.url);
	const response = await event.fetch('/auth/signin/oidc', {
		method: 'POST',
		headers: {
			'Content-Type': 'application/x-www-form-urlencoded',
			'X-Auth-Return-Redirect': '1'
		},
		body: new URLSearchParams({ callbackUrl })
	});

	const payload = (await response.json().catch(() => null)) as { url?: unknown } | null;
	if (!response.ok || typeof payload?.url !== 'string') {
		error(502, 'Unable to start Zitadel sign-in');
	}

	return new Response(null, {
		status: 302,
		headers: { location: payload.url }
	});
};
