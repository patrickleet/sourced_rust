/**
 * Admin resource — all-owners query only.
 * Force-archive: `$lib/api/commands.generated` → `todosForceArchive`.
 */
import { defineResource } from '$lib/gql/define-resource';
import { AdminAllTodosDocument, type AdminAllTodosQuery } from './admin.generated';

export type AdminTodoRow = AdminAllTodosQuery['todos'][number];
export type AdminAllTodosData = AdminAllTodosQuery;

export const adminTodos = defineResource<AdminAllTodosData>({
	query: AdminAllTodosDocument,
	select: (data) => data.todos
});
