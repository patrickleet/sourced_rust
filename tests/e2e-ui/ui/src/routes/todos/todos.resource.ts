/**
 * Co-located todos GraphQL ops — documents from `todos.gql` via codegen.
 * SSR load and browser mutations/refetch import this module only.
 */
import { defineResource } from '$lib/gql/define-resource';
import {
	TodosArchiveDocument,
	TodosCompleteDocument,
	TodosCreateDocument,
	TodosDocument,
	type TodosQuery
} from './todos.generated';

export type TodoRow = TodosQuery['todos'][number];
export type TodosQueryData = TodosQuery;

export const todos = defineResource<
	TodosQueryData,
	{
		create: typeof TodosCreateDocument;
		complete: typeof TodosCompleteDocument;
		archive: typeof TodosArchiveDocument;
	}
>({
	query: TodosDocument,
	mutations: {
		create: TodosCreateDocument,
		complete: TodosCompleteDocument,
		archive: TodosArchiveDocument
	},
	select: (data) => data.todos
});
