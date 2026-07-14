/**
 * Admin all-todos resource — documents from `admin.gql`.
 * Demonstrates role-scoped RLS: admin engine role sees every owner_id.
 */
import { defineResource } from '$lib/gql/define-resource';
import { AdminAllTodosDocument, type AdminAllTodosQuery } from './admin.generated';

export type AdminTodoRow = AdminAllTodosQuery['todos'][number];
export type AdminAllTodosData = AdminAllTodosQuery;

export const adminTodos = defineResource<AdminAllTodosData, Record<string, never>>({
	query: AdminAllTodosDocument,
	select: (data) => data.todos
});
