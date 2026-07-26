/**
 * OIDC authorize scopes for Zitadel (shared by Auth.js config + /signin start).
 *
 * Zitadel uses **project role keys** (not LDAP-style "groups"). When the right
 * reserved scopes are requested, those roles appear on the access/id token as
 * object claims whose keys are the role names (e.g. `admin`).
 */
import { env } from '$env/dynamic/private';
import { cleanEnvValue } from '$lib/clean-env';

const DEFAULT_OIDC_SCOPES = 'openid profile email offline_access';

function envFirst(names: string[], fallback = '') {
	for (const name of names) {
		const value = cleanEnvValue(env[name]);
		if (value) return value;
		// process.env (make run / shell) — same keys, in case kit env is incomplete
		if (typeof process !== 'undefined') {
			const fromProc = cleanEnvValue(process.env[name]);
			if (fromProc) return fromProc;
		}
	}
	return fallback;
}

/**
 * Always merge required Zitadel reserved scopes even if OIDC_SCOPES is incomplete
 * (e.g. bare `openid`). Without project audience + roles scopes, tokens have no
 * role claims even when the human has a project grant and the app has
 * accessTokenRoleAssertion / idTokenRoleAssertion enabled.
 */
export function oidcScopes(): string {
	const fromEnv = envFirst(['OIDC_SCOPES']);
	const aud = envFirst(['OIDC_AUDIENCE', 'ZITADEL_PROJECT_ID']);
	const parts = new Set<string>();

	for (const piece of `${fromEnv || DEFAULT_OIDC_SCOPES}`.split(/\s+/)) {
		if (piece) parts.add(piece);
	}

	for (const s of DEFAULT_OIDC_SCOPES.split(/\s+/)) parts.add(s);

	if (aud) {
		parts.add(`urn:zitadel:iam:org:project:id:${aud}:aud`);
		parts.add('urn:zitadel:iam:org:project:roles');
		parts.add('urn:zitadel:iam:org:projects:roles');
		parts.add(`urn:zitadel:iam:org:project:id:${aud}:roles`);
	}

	return [...parts].join(' ');
}

export function oidcAudience(): string {
	return envFirst(['OIDC_AUDIENCE', 'ZITADEL_PROJECT_ID']);
}
