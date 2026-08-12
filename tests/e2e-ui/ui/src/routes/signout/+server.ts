import { redirect } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import { env } from '$env/dynamic/private';
import { deleteAuthCookies } from '$lib/server/auth-cookies';

function envFirst(names: string[]) {
	for (const name of names) {
		const value = env[name]?.trim();
		if (value) return value;
	}

	return undefined;
}

function oidcIssuer() {
	return envFirst(['OIDC_ISSUER', 'ZITADEL_ISSUER'])?.replace(/\/+$/, '');
}

async function endSessionEndpoint() {
	const override = envFirst(['OIDC_END_SESSION_ENDPOINT']);
	if (override) return override;

	const issuer = oidcIssuer();
	if (!issuer) return undefined;

	try {
		const response = await fetch(`${issuer}/.well-known/openid-configuration`);
		if (!response.ok) return undefined;

		const metadata = (await response.json()) as { end_session_endpoint?: string };
		return metadata.end_session_endpoint;
	} catch {
		return undefined;
	}
}

export const GET: RequestHandler = async (event) => {
	const session = await event.locals.auth();
	const idToken = session?.idToken;

	deleteAuthCookies(event.cookies);

	const logoutEndpoint = await endSessionEndpoint();
	if (!logoutEndpoint) {
		redirect(303, '/');
	}

	const endSessionUrl = new URL(logoutEndpoint);
	if (idToken) endSessionUrl.searchParams.set('id_token_hint', idToken);
	// Zitadel exact-matches post_logout_redirect_uri against app allowlist
	// (bootstrap registers origin + trailing slash, e.g. http://127.0.0.1:5180/).
	// event.url.origin has no path/slash — bare origin is rejected as invalid.
	endSessionUrl.searchParams.set('post_logout_redirect_uri', `${event.url.origin}/`);

	redirect(303, endSessionUrl.toString());
};
