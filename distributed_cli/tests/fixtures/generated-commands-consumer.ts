import {
	COMMAND_ARTIFACTS,
	COMMANDS,
	Command_projectTodo,
	createCommands,
	prepareCommand_projectTodo,
	type Command_projectTodo_Input,
	type Command_projectTodo_Output
} from './generated-commands.js';
import type { ReplicaCommandArtifact } from '@hops-ops/distributed/replica';

const artifact: ReplicaCommandArtifact<
	Command_projectTodo_Input,
	Command_projectTodo_Output
> = Command_projectTodo;
const artifactVersion: 2 = artifact.version;
const inventoryVersion: 2 = COMMANDS['todo.project'].version;
const input: Command_projectTodo_Input = {
	id: 'todo-1',
	tenantId: 'tenant-1'
};
const artifactCount: number = COMMAND_ARTIFACTS.length;

void [
	artifactVersion,
	inventoryVersion,
	input,
	artifactCount,
	createCommands,
	prepareCommand_projectTodo
];
