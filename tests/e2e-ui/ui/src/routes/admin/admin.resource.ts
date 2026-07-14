/**
 * Admin resource — all-owners query + admin-only force-archive mutation.
 * Mutation document is only valid against role `admin` SDL.
 */
import { defineResource } from '$lib/gql/define-resource';
import {
	AdminAllTodosDocument,
	AdminForceArchiveDocument,
	type AdminAllTodosQuery
} from './admin.generated';

export type AdminTodoRow = AdminAllTodosQuery['todos'][number];
export type AdminAllTodosData = AdminAllTodosQuery;

export const adminTodos = defineResource<
	AdminAllTodosData,
	{ forceArchive: typeof AdminForceArchiveDocument }
>({
	query: AdminAllTodosDocument,
	mutations: {
		forceArchive: AdminForceArchiveDocument
	},
	select: (data) => data.todos
});
