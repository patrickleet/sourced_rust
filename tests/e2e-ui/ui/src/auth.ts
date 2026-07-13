/**
 * Auth.js (SvelteKit) OIDC — patterns from the-website, without hops branding.
 * Env: OIDC_ISSUER, OIDC_CLIENT_ID, OIDC_CLIENT_SECRET, AUTH_SECRET, optional OIDC_SCOPES.
 */
import { SvelteKitAuth } from '@auth/sveltekit';
import { env } from '$env/dynamic/private';

const DEFAULT_OIDC_SCOPES = 'openid profile email offline_access';
const DEFAULT_GROUP_CLAIMS = ['groups', 'roles', 'urn:zitadel:iam:org:project:roles'];
const ACCESS_TOKEN_REFRESH_SKEW_SECONDS = 60;

type TokenRecord = Record<string, unknown>;

function envFirst(names: string[], fallback = '') {
  for (const name of names) {
    const value = env[name]?.trim();
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
  return envFirst(['OIDC_SCOPES'], DEFAULT_OIDC_SCOPES);
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
      for (const key of Object.keys(value as object)) groups.add(key);
    }
  }
  return [...groups].sort();
}

function userClaims(token: TokenRecord) {
  return decodeJwtPayload(token.idToken) ?? decodeJwtPayload(token.accessToken) ?? {};
}

/** Primary engine role for GraphQL: admin wins, else user. */
export function engineRoleFromGroups(groups: string[] | undefined): 'admin' | 'user' {
  if (groups?.includes('admin')) return 'admin';
  return 'user';
}

const oidcConfigured = Boolean(oidcIssuer() && oidcClientId());

export const { handle, signIn, signOut } = SvelteKitAuth({
  providers: oidcConfigured
    ? [
        {
          id: 'oidc',
          name: env.AUTH_PROVIDER?.trim() || 'OIDC',
          type: 'oidc',
          issuer: oidcIssuer(),
          clientId: oidcClientId(),
          clientSecret: oidcClientSecret() || undefined,
          authorization: { params: { scope: oidcScopes() } },
          checks: ['pkce', 'state'],
          client: { token_endpoint_auth_method: oidcClientSecret() ? 'client_secret_basic' : 'none' },
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
              username: claimString(profile, 'preferred_username')
            };
          }
        } as any
      ]
    : [],
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
      return session;
    },
    async redirect({ url, baseUrl }) {
      if (url.startsWith('/')) return `${baseUrl}${url}`;
      if (new URL(url).origin === baseUrl) return url;
      return baseUrl;
    }
  },
  pages: {
    signIn: '/signin',
    error: '/signin'
  },
  trustHost: true,
  secret: env.AUTH_SECRET || 'dev-only-change-me'
});

async function refreshAccessToken(token: TokenRecord) {
  const issuer = oidcIssuer();
  const discovery = await fetch(`${issuer}/.well-known/openid-configuration`).then((r) => r.json());
  const tokenEndpoint = discovery.token_endpoint as string;
  const body = new URLSearchParams({
    grant_type: 'refresh_token',
    refresh_token: String(token.refreshToken),
    client_id: oidcClientId()
  });
  const headers: Record<string, string> = {
    'Content-Type': 'application/x-www-form-urlencoded'
  };
  const secret = oidcClientSecret();
  if (secret) {
    headers.Authorization = `Basic ${btoa(`${oidcClientId()}:${secret}`)}`;
  }
  const response = await fetch(tokenEndpoint, { method: 'POST', headers, body });
  const refreshed = (await response.json()) as {
    access_token?: string;
    refresh_token?: string;
    id_token?: string;
    expires_in?: number;
  };
  if (!response.ok) throw refreshed;
  const expiresIn = typeof refreshed.expires_in === 'number' ? refreshed.expires_in : 3600;
  return {
    ...token,
    accessToken: refreshed.access_token,
    refreshToken: refreshed.refresh_token ?? token.refreshToken,
    idToken: refreshed.id_token ?? token.idToken,
    expiresAt: Math.floor(Date.now() / 1000) + expiresIn,
    error: undefined
  };
}

export function isOidcConfigured() {
  return oidcConfigured;
}
