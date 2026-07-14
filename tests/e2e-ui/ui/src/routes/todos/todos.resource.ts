/**
 * Co-located todos **query** — documents from `todos.gql` via codegen.
 * Writes use `$lib/api/commands.generated` (same registry as engine).
 */
import { defineResource } from '$lib/gql/define-resource';
import { TodosDocument, type TodosQuery } from './todos.generated';

export type TodoRow = TodosQuery['todos'][number];
export type TodosQueryData = TodosQuery;

export const todos = defineResource<TodosQueryData>({
	query: TodosDocument,
	select: (data) => data.todos
});
