import type { LayoutServerLoad } from './$types';
import { engineRoleFromGroups } from '$lib/roles';

export const load: LayoutServerLoad = async (event) => {
	const session = await event.locals.auth();
	return {
		session,
		/** Engine GraphQL role from OIDC groups (user | admin) — nav + gates. */
		engineRole: engineRoleFromGroups(session?.user?.groups)
	};
};
