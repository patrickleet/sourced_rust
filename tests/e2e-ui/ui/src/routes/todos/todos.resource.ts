/**
 * Co-located todos GraphQL ops — single source of truth for query + mutations.
 * SSR load and browser mutations/refetch import this module only (same document refs).
 */
import { defineResource } from '$lib/gql/define-resource';

export type TodoRow = {
	todo_id: string;
	owner_id: string;
	title: string;
	status: string;
};

export type TodosQueryData = {
	todos: TodoRow[];
};

const TODOS_QUERY = `{
  todos {
    todo_id
    owner_id
    title
    status
  }
}`;

const TODOS_CREATE = `mutation TodosCreate($todo_id: String!, $title: String!) {
  todos_create(input: { todo_id: $todo_id, title: $title }) {
    todo_id
    owner_id
    title
    status
  }
}`;

const TODOS_COMPLETE = `mutation TodosComplete($todo_id: String!) {
  todos_complete(input: { todo_id: $todo_id }) {
    todo_id
    status
  }
}`;

const TODOS_ARCHIVE = `mutation TodosArchive($todo_id: String!) {
  todos_archive(input: { todo_id: $todo_id }) {
    todo_id
    status
  }
}`;

export const todos = defineResource<
	TodosQueryData,
	{
		create: string;
		complete: string;
		archive: string;
	}
>({
	query: TODOS_QUERY,
	mutations: {
		create: TODOS_CREATE,
		complete: TODOS_COMPLETE,
		archive: TODOS_ARCHIVE
	},
	select: (data) => data.todos
});
