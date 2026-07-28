import {
	COMMAND_ARTIFACTS,
	COMMAND_RUNTIME_REQUIRES_ARTIFACT_V2,
	COMMANDS,
	Command_projectTodo,
	type Command_projectTodo_Input,
	type Command_projectTodo_Output,
	type CompilerReplicaCommandArtifactV2
} from './generated-commands.js';

const artifact: CompilerReplicaCommandArtifactV2<
	Command_projectTodo_Input,
	Command_projectTodo_Output
> = Command_projectTodo;
const artifactVersion: 2 = artifact.version;
const inventoryVersion: 2 = COMMANDS['todo.project'].version;
const runtimeDeferred: true = COMMAND_RUNTIME_REQUIRES_ARTIFACT_V2;
const input: Command_projectTodo_Input = {
	id: 'todo-1',
	tenantId: 'tenant-1'
};
const artifactCount: number = COMMAND_ARTIFACTS.length;

void [
	artifactVersion,
	inventoryVersion,
	runtimeDeferred,
	input,
	artifactCount
];
