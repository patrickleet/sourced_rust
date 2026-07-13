/**
 * Pure role helper — safe for browser and server (no $env).
 * Do not import from auth.ts in client components.
 */
export function engineRoleFromGroups(groups: string[] | undefined): 'admin' | 'user' {
	if (groups?.includes('admin') || groups?.includes('admins')) return 'admin';
	return 'user';
}

export function roleFromGroups(groups: string[] | undefined): 'admin' | 'user' {
	return engineRoleFromGroups(groups);
}
