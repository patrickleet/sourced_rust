/**
 * Shared OIDC start for /login, /signin, /signup.
 * Probes the IdP before Auth.js so we never bounce to `/?error=Configuration`
 * with no explanation (common when Zitadel / Docker is down).
 *
 * Uses `redirect()` (throws) so it works from page `load`, form actions, and
 * +server handlers. Returning a raw Response from `load` is invalid in SvelteKit.
 */
import { error, redirect } from '@sveltejs/kit';
import { env } from '$env/dynamic/private';
import { cleanEnvValue } from '$lib/clean-env';
import { oidcScopes } from '$lib/server/oidc-scopes';

export function safeCallbackUrl(url: URL) {
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

function oidcIssuer(): string {
	const raw =
		cleanEnvValue(env.OIDC_ISSUER) ||
		cleanEnvValue(env.ZITADEL_ISSUER) ||
		'';
	return raw.replace(/\/+$/, '');
}

/**
 * Fail fast with a actionable message if OIDC is not configured or the IdP
 * (Zitadel on :18080) is unreachable.
 */
export async function assertOidcReady(): Promise<void> {
	const issuer = oidcIssuer();
	const clientId = cleanEnvValue(env.OIDC_CLIENT_ID) || cleanEnvValue(env.ZITADEL_CLIENT_ID);

	if (!issuer || !clientId) {
		error(
			503,
			'OIDC is not configured. From tests/e2e-ui run: make up  (writes e2e-ui.env with OIDC_*). Then restart the UI with that env sourced.'
		);
	}

	const discovery = `${issuer}/.well-known/openid-configuration`;
	try {
		const res = await fetch(discovery, {
			signal: AbortSignal.timeout(2500)
		});
		if (!res.ok) {
			error(
				503,
				`Identity provider at ${issuer} returned HTTP ${res.status}. Start Zitadel: cd tests/e2e-ui && make up  (needs Docker/Colima). Demo logins after up: alice / bob / admin · Password1!`
			);
		}
	} catch {
		error(
			503,
			`Cannot reach identity provider at ${issuer} (Docker/Zitadel not running). Start it with: cd tests/e2e-ui && make up  Then source e2e-ui.env and restart make run / the UI. Until then use demo users after the stack is up — self-registration is enabled by bootstrap when Zitadel is healthy.`
		);
	}
}

export async function startOidcSignIn(
	event: { fetch: typeof fetch; url: URL },
	label: 'sign-in' | 'sign-up' = 'sign-in'
): Promise<never> {
	await assertOidcReady();

	const callbackUrl = safeCallbackUrl(event.url);
	// Pass scope explicitly so Auth.js authorization params include Zitadel role
	// scopes even if the provider config was frozen with a bare openid default.
	const response = await event.fetch('/auth/signin/oidc', {
		method: 'POST',
		headers: {
			'Content-Type': 'application/x-www-form-urlencoded',
			'X-Auth-Return-Redirect': '1'
		},
		body: new URLSearchParams({
			callbackUrl,
			scope: oidcScopes()
		})
	});

	const payload = (await response.json().catch(() => null)) as { url?: unknown } | null;
	if (!response.ok || typeof payload?.url !== 'string') {
		error(
			502,
			`Unable to start Zitadel ${label}. Check OIDC_CLIENT_ID / OIDC_CLIENT_SECRET in e2e-ui.env (re-run make up if the secret file is missing).`
		);
	}

	// Throws — valid from load / actions / +server
	redirect(302, payload.url);
}
