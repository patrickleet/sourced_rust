import {
	createCommands,
	type GeneratedCommands,
	type GeneratedCommandRuntime
} from './generated-commands.js';
import type {
	DistributedReplica,
	ReplicaCommandTransport
} from '@hops-ops/distributed/replica';

declare const replica: DistributedReplica;
declare const transport: ReplicaCommandTransport;

const runtime: GeneratedCommandRuntime = createCommands(replica, transport);
const commands: GeneratedCommands = runtime.commands;

commands.todo.import({ source: 'fixture' });
commands.todo.ping();
commands.todo.project({ id: 'todo-1', tenantId: 'tenant-1' }).then(
	(receipt) => {
		const title: string | null = receipt.result.title;
		return title;
	}
);

runtime.observeResult({});
runtime.dispose();

// @ts-expect-error Generated object inputs remain exact and required.
commands.todo.project({ id: 'todo-1' });
// @ts-expect-error A no-input command accepts options, not domain input.
commands.todo.ping({ id: 'todo-1' });
// @ts-expect-error Descriptor inventory is not presented as callable commands.
createCommands(replica, transport).commands.todo.project.artifact;
