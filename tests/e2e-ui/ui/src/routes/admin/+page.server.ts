import { error } from '@sveltejs/kit';
import { loadQuery } from '$lib/gql/load-query.server';
import { engineRoleFromGroups } from '$lib/roles';
import { adminTodos } from './admin.resource';
import type { AdminAllTodosData } from './admin.resource';
import type { PageServerLoad } from './$types';

/**
 * Admin-only SSR seed: full todos list (no owner filter when engineRole is admin).
 * Non-admins get 403 — do not leak the all-owners view through this route.
 */
export const load: PageServerLoad = async (event) => {
	const session = await event.locals.auth();
	const engineRole = engineRoleFromGroups(session?.user?.groups);
	if (engineRole !== 'admin') {
		error(403, 'Admin role required — sign in as admin (Zitadel: admin / Password1!)');
	}

	const seeded = await loadQuery<AdminAllTodosData, { todos: AdminAllTodosData['todos'] }>(
		adminTodos.query,
		(data) => ({
			todos: data?.todos ?? []
		})
	)(event);

	return {
		...seeded,
		/** Explicit for the page copy — always admin when load succeeds. */
		isAdminView: true as const
	};
};
