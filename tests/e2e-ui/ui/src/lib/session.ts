export { engineRoleFromGroups, roleFromGroups } from './roles';

type SessionLike = {
	user?: {
		id?: string | null;
		username?: string | null;
		name?: string | null;
		email?: string | null;
	} | null;
	accessToken?: string | null;
} | null | undefined;

/** Display label for UI chrome (username → name → email → fallback). */
export function sessionDisplayName(session: SessionLike, fallback = 'you'): string {
	return (
		session?.user?.username ?? session?.user?.name ?? session?.user?.email ?? fallback
	);
}

/**
 * Decode JWT `sub` without verification (UI identity only).
 * Prefer this over `session.user.id` when matching server `author_id` /
 * `owner_id` — commands use the access-token principal, which can diverge
 * from Auth.js `token.sub` after refresh or IdP claim quirks.
 */
export function accessTokenSub(accessToken: string | null | undefined): string | null {
	if (!accessToken) return null;
	const parts = accessToken.split('.');
	if (parts.length < 2) return null;
	try {
		const b64 = parts[1].replace(/-/g, '+').replace(/_/g, '/');
		const pad = b64.length % 4 === 0 ? '' : '='.repeat(4 - (b64.length % 4));
		const json = JSON.parse(atob(b64 + pad)) as { sub?: unknown };
		return typeof json.sub === 'string' && json.sub.length > 0 ? json.sub : null;
	} catch {
		return null;
	}
}

/** Principal id used for “is this message mine?” (access token sub → session id). */
export function sessionPrincipalId(
	session: SessionLike,
	accessToken?: string | null
): string {
	return (
		accessTokenSub(accessToken ?? session?.accessToken) ??
		session?.user?.id?.trim() ??
		''
	);
}

/** True when a message/game row belongs to the signed-in principal. */
export function isOwnAuthor(
	authorId: string | null | undefined,
	principalId: string,
	opts?: { authorUserId?: string | null; username?: string | null; displayName?: string | null }
): boolean {
	const me = principalId.trim();
	if (!me) return false;
	if (authorId && authorId === me) return true;
	if (opts?.authorUserId && opts.authorUserId === me) return true;
	// Soft match: joined display name equals preferred_username (scrape / IdP label).
	const uname = opts?.username?.trim().toLowerCase();
	const label = opts?.displayName?.trim().toLowerCase();
	if (uname && label && uname === label) return true;
	return false;
}
