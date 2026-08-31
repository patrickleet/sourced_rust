import {
	bindSveltekitOperation,
	createDistributedSvelteKit,
	createDistributedSvelteKitServer
} from '@hops-ops/distributed/sveltekit';
import { distributedSvelteKit } from '@hops-ops/distributed/sveltekit/vite';
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

const standaloneTodos = bindSveltekitOperation(replica, Todos).use();
standaloneTodos.subscribe((snapshot) => {
	if (snapshot.complete) {
		snapshot.data.todos.map((todo) => todo.title);
	} else {
		snapshot.data.todos?.map((todo) => todo?.title);
	}
});
void standaloneTodos.refetch();

const client = createDistributedSvelteKit({
	session: {
		getAuth: () => ({ accessToken: 'token' })
	},
	createCommands: () => generatedRuntime
});
const todos = client.operation(Todos).use({}, { live: true });
const selected = client.operation(TodoById).use({ id: 'todo-1' });
selected.data.todo?.status;
todos.pending.map((receipt) => receipt.status());

void client.commands.todo.complete({ todoId: 'todo-1' });
// @ts-expect-error Generated command input remains exact through Svelte usage.
void client.commands.todo.complete({ id: 'todo-1' });

// @ts-expect-error Required generated operation variables cannot be omitted.
client.operation(TodoById).use();
// @ts-expect-error Unknown generated operation variables fail at compile time.
client.operation(TodoById).use({ id: 'todo-1', forged: true });

const todoBoundary = client.operation(TodoById).boundary<
	Readonly<{ user: Readonly<{ id: string }> }>,
	Readonly<{ forwardedId: string }>
>(
	{
		operation: 'TodoById',
		route: '/todos/[id]',
		kind: 'page',
		discovery: 'component'
	},
	{ id: { kind: 'route_param', name: 'id' } }
);
todoBoundary.binding.resolve({
	params: { id: 'todo-1' },
	search: new URLSearchParams(),
	session: { user: { id: 'user-1' } },
	props: { forwardedId: 'todo-1' }
}).id satisfies string;

client.operation(TodoById).boundary(
	{
		operation: 'TodoById',
		route: '/todos/[id]',
		kind: 'page',
		discovery: 'component'
	},
	{
		// @ts-expect-error Constants retain the generated GraphQL variable type.
		id: { kind: 'constant', value: 42 }
	}
);

createDistributedSvelteKitServer({
	routes: [
		{
			plan: {
				operation: 'Todos',
				route: '/todos',
				discovery: 'convention'
			},
			artifact: Todos
		}
	] as const,
	getSession: async () => null,
	getRole: () => 'user'
});

const vitePlugin = distributedSvelteKit({
	clients: [
		{
			module: '$distributed',
			manifest: 'target/distributed-client.json',
			role: 'user',
			documents: ['src/**/*.graphql'],
			out: 'src/lib/generated/distributed'
		}
	]
});
vitePlugin.configureServer({
	watcher: { add: () => undefined },
	ws: { send: () => undefined },
	moduleGraph: {
		getModuleById: () => undefined,
		invalidateModule: () => undefined
	},
	httpServer: null
});
