/**
 * Zitadel Login V2 helpers for custom auth pages.
 *
 * Auth.js still starts OIDC (PKCE). After authorize, Zitadel redirects to our
 * /login?authRequest=V2_…  We authenticate via Session API v2, then
 * CreateCallback to get the OIDC code URL back to Auth.js.
 *
 * Requires server-only ZITADEL_SERVICE_USER_TOKEN (IAM_LOGIN_CLIENT PAT from make up).
 */
import { env } from '$env/dynamic/private';
import { cleanEnvValue } from '$lib/clean-env';

export class ZitadelAuthError extends Error {
	constructor(
		message: string,
		public readonly status = 400,
		public readonly code?: string
	) {
		super(message);
		this.name = 'ZitadelAuthError';
	}
}

function issuer(): string {
	const raw =
		cleanEnvValue(env.OIDC_ISSUER) ||
		cleanEnvValue(env.ZITADEL_ISSUER) ||
		'';
	return raw.replace(/\/+$/, '');
}

function serviceToken(): string {
	return (
		cleanEnvValue(env.ZITADEL_SERVICE_USER_TOKEN) ||
		cleanEnvValue(env.ZITADEL_LOGIN_CLIENT_TOKEN) ||
		''
	);
}

function projectId(): string {
	return cleanEnvValue(env.OIDC_AUDIENCE) || cleanEnvValue(env.ZITADEL_PROJECT_ID) || '';
}

export function assertLoginClientConfigured(): void {
	if (!issuer()) {
		throw new ZitadelAuthError(
			'OIDC_ISSUER is not set. Run make up and source e2e-ui.env.',
			503
		);
	}
	if (!serviceToken()) {
		throw new ZitadelAuthError(
			'ZITADEL_SERVICE_USER_TOKEN missing. Re-run make up (writes login-client PAT) and restart the UI.',
			503
		);
	}
}

async function zitadelFetch(
	path: string,
	init: RequestInit & { method?: string } = {}
): Promise<{ ok: boolean; status: number; body: Record<string, unknown> }> {
	const base = issuer();
	const token = serviceToken();
	const res = await fetch(`${base}${path}`, {
		...init,
		headers: {
			Authorization: `Bearer ${token}`,
			'Content-Type': 'application/json',
			Accept: 'application/json',
			...(init.headers ?? {})
		},
		signal: AbortSignal.timeout(12_000)
	});
	const text = await res.text();
	let body: Record<string, unknown> = {};
	if (text) {
		try {
			body = JSON.parse(text) as Record<string, unknown>;
		} catch {
			body = { message: text.slice(0, 400) };
		}
	}
	return { ok: res.ok, status: res.status, body };
}

function apiErrorMessage(body: Record<string, unknown>, fallback: string): string {
	const msg = body.message ?? body.error_description ?? body.error;
	if (typeof msg === 'string' && msg.trim()) return msg.trim();
	return fallback;
}

export type PasswordSession = {
	sessionId: string;
	sessionToken: string;
};

/** Create a Zitadel session with login name + password checks (Session API v2). */
export async function createPasswordSession(
	loginName: string,
	password: string
): Promise<PasswordSession> {
	assertLoginClientConfigured();
	const name = loginName.trim();
	if (!name || !password) {
		throw new ZitadelAuthError('Username and password are required.');
	}

	const { ok, status, body } = await zitadelFetch('/v2/sessions', {
		method: 'POST',
		body: JSON.stringify({
			checks: {
				user: { loginName: name },
				password: { password }
			}
		})
	});

	if (!ok) {
		// Wrong password / unknown user often 400 or 404
		const hint =
			status === 404 || status === 400
				? 'Invalid username or password.'
				: apiErrorMessage(body, `Sign-in failed (HTTP ${status}).`);
		throw new ZitadelAuthError(hint, status === 401 || status === 403 ? status : 400);
	}

	const sessionId = body.sessionId;
	const sessionToken = body.sessionToken;
	if (typeof sessionId !== 'string' || typeof sessionToken !== 'string') {
		throw new ZitadelAuthError('Session response missing id/token.', 502);
	}
	return { sessionId, sessionToken };
}

/**
 * Finalize OIDC auth request with the session → callback URL including code
 * for Auth.js (/auth/callback/oidc?code=…&state=…).
 */
export async function finalizeAuthRequest(
	authRequestId: string,
	session: PasswordSession
): Promise<string> {
	assertLoginClientConfigured();
	const id = authRequestId.trim();
	if (!id) {
		throw new ZitadelAuthError('Missing authRequest. Start sign-in again.');
	}

	const { ok, status, body } = await zitadelFetch(
		`/v2/oidc/auth_requests/${encodeURIComponent(id)}`,
		{
			method: 'POST',
			body: JSON.stringify({
				session: {
					sessionId: session.sessionId,
					sessionToken: session.sessionToken
				}
			})
		}
	);

	if (!ok) {
		throw new ZitadelAuthError(
			apiErrorMessage(body, `Could not complete sign-in (HTTP ${status}).`),
			status >= 500 ? 502 : 400
		);
	}

	const callbackUrl = body.callbackUrl;
	if (typeof callbackUrl !== 'string' || !callbackUrl) {
		throw new ZitadelAuthError('OIDC callback URL missing from Zitadel response.', 502);
	}
	return callbackUrl;
}

export type RegisterInput = {
	username: string;
	email: string;
	password: string;
	givenName?: string;
	familyName?: string;
};

/** Create a human user (User v2) and grant the project `user` role. */
export async function registerHuman(input: RegisterInput): Promise<{ userId: string }> {
	assertLoginClientConfigured();
	const username = input.username.trim();
	const email = input.email.trim();
	const password = input.password;
	if (!username || !email || !password) {
		throw new ZitadelAuthError('Username, email, and password are required.');
	}
	if (password.length < 8) {
		throw new ZitadelAuthError('Password must be at least 8 characters.');
	}

	const given = (input.givenName?.trim() || username).slice(0, 200);
	const family = (input.familyName?.trim() || 'User').slice(0, 200);

	const create = await zitadelFetch('/v2/users/human', {
		method: 'POST',
		body: JSON.stringify({
			username,
			profile: {
				givenName: given,
				familyName: family,
				displayName: username
			},
			email: {
				email,
				isVerified: true
			},
			password: {
				password,
				changeRequired: false
			}
		})
	});

	if (!create.ok) {
		const msg = apiErrorMessage(create.body, `Registration failed (HTTP ${create.status}).`);
		// Duplicate username/email
		throw new ZitadelAuthError(msg, create.status === 409 ? 409 : 400);
	}

	const userId = create.body.userId;
	if (typeof userId !== 'string' || !userId) {
		throw new ZitadelAuthError('Registration response missing userId.', 502);
	}

	const pid = projectId();
	if (pid) {
		const grant = await zitadelFetch(`/management/v1/users/${userId}/grants`, {
			method: 'POST',
			body: JSON.stringify({
				projectId: pid,
				roleKeys: ['user']
			})
		});
		// 409 already granted is fine; other errors are soft (user can still log in)
		if (!grant.ok && grant.status !== 409) {
			console.warn(
				'[zitadel-session] project grant failed',
				grant.status,
				apiErrorMessage(grant.body, '')
			);
		}
	}

	return { userId };
}

/** Password login + OIDC finalize in one step. */
export async function loginWithPassword(
	authRequestId: string,
	loginName: string,
	password: string
): Promise<string> {
	const session = await createPasswordSession(loginName, password);
	return finalizeAuthRequest(authRequestId, session);
}

/** Register, then password-login into the pending auth request. */
export async function registerAndLogin(
	authRequestId: string,
	input: RegisterInput
): Promise<string> {
	await registerHuman(input);
	return loginWithPassword(authRequestId, input.username, input.password);
}
