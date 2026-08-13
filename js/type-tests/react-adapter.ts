import { createElement } from 'react';

import {
	DistributedProvider,
	useDistributedQuery
} from '@hops-ops/distributed/react';
import type {
	DistributedReplica,
	ReplicaCommandArtifact,
	ReplicaCommandRuntime,
	ReplicaOperationArtifact
} from '@hops-ops/distributed/replica';

type Todo = {
	id: string;
	title: string;
	status: 'open' | 'done';
};
type TodosResult = { todos: readonly Todo[] };
type TodoResult = { todo: Todo | null };

declare const replica: DistributedReplica;
declare const Todos: ReplicaOperationArtifact<
	TodosResult,
	Readonly<Record<never, never>>
>;
declare const TodoById: ReplicaOperationArtifact<
	TodoResult,
	Readonly<{ id: string }>
>;
declare const generatedRuntime: ReplicaCommandRuntime<{
	'todo.complete': ReplicaCommandArtifact<
		Readonly<{ todoId: string }>,
		Readonly<{ accepted: boolean }>
	>;
}>;

function TodosView() {
	const todos = useDistributedQuery(Todos);
	const selected = useDistributedQuery(TodoById, { id: 'todo-1' }, { live: true });
	void todos.refresh();

	if (todos.complete) {
		todos.data.todos.map((todo) => todo.title);
	} else {
		todos.data.todos?.map((todo) => todo?.title);
	}
	if (selected.complete) {
		selected.data.todo?.status;
	}

	void generatedRuntime.commands.todo.complete({ todoId: 'todo-1' });
	// @ts-expect-error Generated command input remains exact through React usage.
	void generatedRuntime.commands.todo.complete({ id: 'todo-1' });
	return null;
}

createElement(
	DistributedProvider,
	{ replica },
	createElement(TodosView)
);

// @ts-expect-error Required generated operation variables cannot be omitted.
useDistributedQuery(TodoById);
// @ts-expect-error Unknown generated operation variables fail at compile time.
useDistributedQuery(TodoById, { id: 'todo-1', forged: true });
