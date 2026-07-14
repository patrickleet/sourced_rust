export { engineRoleFromGroups, roleFromGroups } from './roles';

type SessionLike = {
	user?: {
		id?: string | null;
		username?: string | null;
		name?: string | null;
		email?: string | null;
	} | null;
} | null;

/** Display label for UI chrome (username → name → email → fallback). */
export function sessionDisplayName(session: SessionLike, fallback = 'you'): string {
	return (
		session?.user?.username ?? session?.user?.name ?? session?.user?.email ?? fallback
	);
}
