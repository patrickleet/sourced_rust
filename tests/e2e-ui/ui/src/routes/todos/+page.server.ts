import { loadQuery } from '$lib/gql/load-query.server';
import { todos } from './todos.resource';
import type { TodosQueryData } from './todos.resource';

/**
 * SSR seed only — same `todos.query` document the browser refetches.
 * Mutations run in the browser via useGraphql → POST /graphql.
 */
export const load = loadQuery<TodosQueryData, { todos: TodosQueryData['todos'] }>(
	todos.query,
	(data) => ({
		todos: data?.todos ?? []
	})
);
