/**
 * Admin resource — all-owners query only.
 * Force-archive: `$lib/api/commands.generated` → `todosForceArchive`.
 */
import { defineResource } from '@hops-ops/distributed';
import { AdminAllTodosDocument, type AdminAllTodosQuery } from './admin.generated';

export type AdminTodoRow = AdminAllTodosQuery['todos'][number];
export type AdminAllTodosData = AdminAllTodosQuery;

/** open → completed → archived, then owner_id, then todo_id (stable). */
export function sortAdminTodos(list: AdminTodoRow[]): AdminTodoRow[] {
	const rank = (s: string) => {
		switch (s) {
			case 'open':
				return 0;
			case 'completed':
				return 1;
			case 'archived':
				return 2;
			default:
				return 3;
		}
	};
	return [...list].sort((a, b) => {
		const byStatus = rank(a.status) - rank(b.status);
		if (byStatus !== 0) return byStatus;
		if (a.owner_id !== b.owner_id) return a.owner_id < b.owner_id ? -1 : 1;
		return a.todo_id < b.todo_id ? -1 : a.todo_id > b.todo_id ? 1 : 0;
	});
}

export const adminTodos = defineResource<AdminAllTodosData>({
	query: AdminAllTodosDocument,
	select: (data) => sortAdminTodos(data.todos ?? [])
});
