import { SvelteKitAuth } from '@auth/sveltekit';
import { env } from '$env/dynamic/private';

declare module '@auth/sveltekit' {
	interface Session {
		accessToken?: string;
		idToken?: string;
		expiresAt?: number;
		refreshAfter?: number;
		hasAccessToken?: boolean;
		hasRefreshToken?: boolean;
		hasIdToken?: boolean;
		error?: string;
	}

	interface User {
		id?: string;
		groups?: string[];
		username?: string;
		emailVerified?: boolean;
	}
}

const DEFAULT_OIDC_SCOPES = 'openid profile email offline_access';
const DEFAULT_GROUP_CLAIMS = ['groups', 'roles', 'urn:zitadel:iam:org:project:roles'];
const ACCESS_TOKEN_REFRESH_SKEW_SECONDS = 60;
const TOKEN_AUTH_BASIC = 'client_secret_basic';
const TOKEN_AUTH_POST = 'client_secret_post';
const TOKEN_AUTH_NONE = 'none';
const DEFAULT_SESSION_COOKIE_SAME_SITE = 'strict';

type TokenRecord = Record<string, unknown>;
type SessionCookieSameSite = 'lax' | 'strict';

/** Peel accidental outer quotes (Make-include / double-wrap pollution). */
function cleanEnvValue(raw: string | undefined): string {
	let s = (raw ?? '').trim();
	for (let i = 0; i < 2; i++) {
		if (
			s.length >= 2 &&
			((s.startsWith("'") && s.endsWith("'")) || (s.startsWith('"') && s.endsWith('"')))
		) {
			s = s.slice(1, -1).trim();
		} else {
			break;
		}
	}
	return s;
}

function envFirst(names: string[], fallback = '') {
	for (const name of names) {
		const value = cleanEnvValue(env[name]);
		if (value) return value;
	}

	return fallback;
}

function envCsv(name: string, fallback: string[]) {
	const value = env[name]?.trim();
	if (!value) return fallback;

	const items = value
		.split(',')
		.map((item) => item.trim())
		.filter(Boolean);

	return items.length ? items : fallback;
}

function oidcIssuer() {
	return envFirst(['OIDC_ISSUER', 'ZITADEL_ISSUER']).replace(/\/+$/, '');
}

function oidcClientId() {
	return envFirst(['OIDC_CLIENT_ID', 'ZITADEL_CLIENT_ID']);
}

function oidcClientSecret() {
	return envFirst(['OIDC_CLIENT_SECRET', 'ZITADEL_CLIENT_SECRET']);
}

function oidcScopes() {
	const fromEnv = envFirst(['OIDC_SCOPES']);
	if (fromEnv) return fromEnv;
	// Ensure project-scoped audience on access tokens when OIDC_AUDIENCE is known.
	const aud = envFirst(['OIDC_AUDIENCE', 'ZITADEL_PROJECT_ID']);
	if (aud) {
		return `${DEFAULT_OIDC_SCOPES} urn:zitadel:iam:org:project:id:${aud}:aud urn:zitadel:iam:org:project:roles`;
	}
	return DEFAULT_OIDC_SCOPES;
}

function oidcTokenAuthMethod() {
	return envFirst(['OIDC_TOKEN_AUTH_METHOD'], TOKEN_AUTH_BASIC).toLowerCase();
}

function authSessionCookieSameSite(): SessionCookieSameSite {
	const value = env.AUTH_SESSION_COOKIE_SAME_SITE?.trim().toLowerCase();
	if (value === 'lax' || value === 'strict') return value;

	if (value) {
		console.warn('AUTH_SESSION_COOKIE_SAME_SITE must be lax or strict; defaulting to strict');
	}

	return DEFAULT_SESSION_COOKIE_SAME_SITE;
}

function providerName() {
	if (env.AUTH_PROVIDER?.trim()) return env.AUTH_PROVIDER.trim();
	if (env.ZITADEL_ISSUER?.trim()) return 'Zitadel';
	return 'OIDC';
}

function decodeJwtPayload(jwt: unknown): Record<string, unknown> | null {
	if (typeof jwt !== 'string') return null;

	const payload = jwt.split('.')[1];
	if (!payload) return null;

	try {
		const normalized = payload.replace(/-/g, '+').replace(/_/g, '/');
		const padded = normalized.padEnd(Math.ceil(normalized.length / 4) * 4, '=');
		const binary = globalThis.atob(padded);
		const bytes = Uint8Array.from(binary, (char) => char.charCodeAt(0));
		return JSON.parse(new TextDecoder().decode(bytes)) as Record<string, unknown>;
	} catch {
		return null;
	}
}

function claimString(claims: Record<string, unknown>, key: string) {
	const value = claims[key];
	return typeof value === 'string' && value.trim() ? value : undefined;
}

function claimBool(claims: Record<string, unknown>, key: string) {
	const value = claims[key];
	return typeof value === 'boolean' ? value : undefined;
}

function claimValue(claims: Record<string, unknown>, path: string): unknown {
	if (path in claims) return claims[path];

	return path.split('.').reduce<unknown>((current, segment) => {
		if (current && typeof current === 'object' && segment in current) {
			return (current as Record<string, unknown>)[segment];
		}

		return undefined;
	}, claims);
}

function extractGroups(claims: Record<string, unknown>, groupClaims: string[]) {
	const groups = new Set<string>();

	for (const claim of groupClaims) {
		const value = claimValue(claims, claim);

		if (Array.isArray(value)) {
			for (const item of value) {
				if (typeof item === 'string' && item.trim()) groups.add(item);
			}
		} else if (typeof value === 'string' && value.trim()) {
			groups.add(value);
		} else if (value && typeof value === 'object') {
			for (const key of Object.keys(value)) groups.add(key);
		}
	}

	return [...groups].sort();
}

function userClaims(token: TokenRecord) {
	return decodeJwtPayload(token.idToken) ?? decodeJwtPayload(token.accessToken) ?? {};
}

export const { handle, signIn, signOut } = SvelteKitAuth({
	providers: [
		{
			id: 'oidc',
			name: providerName(),
			type: 'oidc',
			issuer: oidcIssuer(),
			clientId: oidcClientId(),
			clientSecret: oidcClientSecret(),
			authorization: {
				params: {
					scope: oidcScopes()
				}
			},
			checks: ['pkce', 'state'],
			profile(profile: Record<string, unknown>) {
				const groupClaims = envCsv('OIDC_GROUP_CLAIMS', DEFAULT_GROUP_CLAIMS);
				const name =
					claimString(profile, 'name') ??
					claimString(profile, 'preferred_username') ??
					claimString(profile, 'email');

				return {
					id: claimString(profile, 'sub') ?? '',
					name,
					email: claimString(profile, 'email'),
					image: claimString(profile, 'picture'),
					groups: extractGroups(profile, groupClaims),
					username: claimString(profile, 'preferred_username'),
					emailVerified: claimBool(profile, 'email_verified')
				};
			}
		} as any
	],
	callbacks: {
		async jwt({ token, account }) {
			if (account) {
				token.accessToken = account.access_token;
				token.refreshToken = account.refresh_token;
				token.idToken = account.id_token;
				token.expiresAt =
					(account.expires_at as number | undefined) ??
					Math.floor(Date.now() / 1000) + ((account.expires_in as number | undefined) ?? 3600);
			}

			const expiresAt = typeof token.expiresAt === 'number' ? token.expiresAt : 0;
			if (expiresAt && Date.now() < (expiresAt - ACCESS_TOKEN_REFRESH_SKEW_SECONDS) * 1000) {
				return token;
			}

			if (token.refreshToken) {
				try {
					return await refreshAccessToken(token as TokenRecord);
				} catch (error) {
					console.error('Token refresh failed:', error);
					token.error = 'RefreshAccessTokenError';
					return token;
				}
			}

			return token;
		},
		async session({ session, token }) {
			session.accessToken = token.accessToken as string | undefined;
			session.idToken = token.idToken as string | undefined;
			session.expiresAt = token.expiresAt as number | undefined;
			session.refreshAfter = Math.max(
				0,
				((token.expiresAt as number | undefined) ?? 0) - ACCESS_TOKEN_REFRESH_SKEW_SECONDS
			);
			session.hasAccessToken = typeof token.accessToken === 'string' && token.accessToken.length > 0;
			session.hasRefreshToken =
				typeof token.refreshToken === 'string' && token.refreshToken.length > 0;
			session.hasIdToken = typeof token.idToken === 'string' && token.idToken.length > 0;
			session.error = token.error as string | undefined;
			session.user = {
				...session.user,
				id: token.sub as string
			};

			const claims = userClaims(token as TokenRecord);
			const groups = extractGroups(claims, envCsv('OIDC_GROUP_CLAIMS', DEFAULT_GROUP_CLAIMS));
			if (groups.length) session.user.groups = groups;

			const username = claimString(claims, 'preferred_username');
			if (username) session.user.username = username;

			const emailVerified = claimBool(claims, 'email_verified');
			if (emailVerified !== undefined) {
				(session.user as unknown as { emailVerified?: boolean }).emailVerified = emailVerified;
			}

			return session;
		},
		async redirect({ url, baseUrl }) {
			if (url.startsWith('/')) return `${baseUrl}${url}`;
			if (new URL(url).origin === baseUrl) return url;
			return baseUrl;
		}
	},
	pages: {
		signIn: '/',
		error: '/'
	},
	cookies: {
		sessionToken: {
			options: {
				sameSite: authSessionCookieSameSite()
			}
		}
	},
	jwt: {
		maxAge: 60 * 60
	},
	trustHost: true
});

async function refreshAccessToken(token: TokenRecord) {
	const tokenEndpoint = await oidcTokenEndpoint();
	const tokenAuthMethod = oidcTokenAuthMethod();
	const body = new URLSearchParams({
		grant_type: 'refresh_token',
		refresh_token: String(token.refreshToken)
	});
	const headers: Record<string, string> = {
		'Content-Type': 'application/x-www-form-urlencoded'
	};

	if (tokenAuthMethod === TOKEN_AUTH_BASIC) {
		headers.Authorization = `Basic ${btoa(`${oidcClientId()}:${oidcClientSecret()}`)}`;
	} else if (tokenAuthMethod === TOKEN_AUTH_POST) {
		body.set('client_id', oidcClientId());
		body.set('client_secret', oidcClientSecret());
	} else if (tokenAuthMethod === TOKEN_AUTH_NONE) {
		body.set('client_id', oidcClientId());
	} else {
		throw new Error('OIDC_TOKEN_AUTH_METHOD must be client_secret_basic, client_secret_post, or none');
	}

	const response = await fetch(tokenEndpoint, {
		method: 'POST',
		headers,
		body
	});

	const refreshedTokens = (await response.json()) as {
		access_token?: string;
		refresh_token?: string;
		id_token?: string;
		expires_in?: number;
	};
	if (!response.ok) throw refreshedTokens;

	const expiresIn = typeof refreshedTokens.expires_in === 'number' ? refreshedTokens.expires_in : 3600;

	return {
		...token,
		accessToken: refreshedTokens.access_token,
		refreshToken: refreshedTokens.refresh_token ?? token.refreshToken,
		idToken: refreshedTokens.id_token ?? token.idToken,
		expiresAt: Math.floor(Date.now() / 1000) + expiresIn,
		error: undefined
	};
}

async function oidcTokenEndpoint() {
	const override = envFirst(['OIDC_TOKEN_ENDPOINT']);
	if (override) return override;

	const issuer = oidcIssuer();
	const response = await fetch(`${issuer}/.well-known/openid-configuration`);
	if (!response.ok) {
		throw new Error(`OIDC discovery failed with ${response.status}`);
	}

	const metadata = (await response.json()) as { token_endpoint?: string };
	if (!metadata.token_endpoint) {
		throw new Error('OIDC discovery did not include token_endpoint');
	}

	return metadata.token_endpoint;
}

export { engineRoleFromGroups } from './lib/roles';
