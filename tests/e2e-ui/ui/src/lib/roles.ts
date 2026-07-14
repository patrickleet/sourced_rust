/**
 * Pure role helper — safe for browser and server (no $env).
 * Do not import from auth.ts in client components.
 *
 * Exact group key membership only (not substring). "administrator" is NOT admin.
 * Matches Zitadel project role keys `admin` / `admins` used in e2e bootstrap.
 */
export function engineRoleFromGroups(groups: string[] | undefined): 'admin' | 'user' {
	if (!groups?.length) return 'user';
	if (groups.includes('admin') || groups.includes('admins')) return 'admin';
	return 'user';
}

export function roleFromGroups(groups: string[] | undefined): 'admin' | 'user' {
	return engineRoleFromGroups(groups);
}

/** UI / load gate: engine role must be admin (after engineRoleFromGroups). */
export function isAdminEngineRole(role: string | null | undefined): boolean {
	return role === 'admin';
}
