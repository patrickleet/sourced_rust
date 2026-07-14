import { error } from '@sveltejs/kit';
import { loadQuery } from '$lib/gql/load-query.server';
import { engineRoleFromGroups, isAdminEngineRole } from '$lib/roles';
import { adminTodos } from './admin.resource';
import type { AdminAllTodosData } from './admin.resource';
import type { PageServerLoad } from './$types';

/**
 * Admin-only SSR seed: todos list without owner filter when engineRole is admin.
 * Non-admins get 403 **before** any GraphQL load — no foreign-todo SSR payload.
 */
export const load: PageServerLoad = async (event) => {
	const session = await event.locals.auth();
	const engineRole = engineRoleFromGroups(session?.user?.groups);
	// Fail closed before loadQuery so non-admins never receive all-owners data.
	if (!isAdminEngineRole(engineRole)) {
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
		isAdminView: true as const,
		/** Client may show truncation note when list hits query limit (100). */
		listLimit: 100 as const
	};
};
