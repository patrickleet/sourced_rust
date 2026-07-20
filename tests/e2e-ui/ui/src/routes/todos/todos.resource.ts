/**
 * Co-located todos **query** — documents from `todos.gql` via codegen.
 * Writes use `$lib/api/commands.generated` (same registry as engine).
 */
import { defineResource } from '$lib/gql/define-resource';
import { TodosDocument, type TodosQuery } from './todos.generated';

export type TodoRow = TodosQuery['todos'][number];
export type TodosQueryData = TodosQuery;

/** open → completed → archived, then stable todo_id (locale-independent). */
export function sortTodos<T extends { todo_id: string; status: string }>(list: T[]): T[] {
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
		return a.todo_id < b.todo_id ? -1 : a.todo_id > b.todo_id ? 1 : 0;
	});
}

export const todos = defineResource<TodosQueryData>({
	query: TodosDocument,
	select: (data) => sortTodos(data.todos ?? [])
});
