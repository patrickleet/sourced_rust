import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { test } from 'node:test';

import {
	bindCommands,
	defineCommand,
	defineCommands,
	executeCommand
} from '@hops-ops/distributed/commands';
import { QueryCache, cacheKey, runCommandPipeline } from '@hops-ops/distributed/cache';
import {
	buildMutationOperation,
	fieldToFunctionName,
	generateCommandArtifacts,
	generateCommandPoliciesTs,
	parseCodegenManifest,
	parseCommandManifest
} from '@hops-ops/distributed/codegen';

const CREATE_DOCUMENT =
	'mutation Create($input: TodoInput!) { todos_create(input: $input) { id title } }';
const PING_DOCUMENT = 'mutation Ping { ping { ok } }';
const TODOS_DOCUMENT = 'query Todos { todos { id title } }';

const CREATE = defineCommand({
	field: 'todos_create',
	document: CREATE_DOCUMENT,
	hasInput: true,
	roles: ['user']
});
const PING = defineCommand({
	field: 'ping',
	document: PING_DOCUMENT,
	hasInput: false
});

test('direct commands unwrap their field, preserve status, and shape variables by hasInput', async () => {
	const calls = [];
	const client = {
		async request(document, variables) {
			calls.push({ document, variables });
			return String(document).includes('todos_create')
				? {
						data: { todos_create: { id: 'one', title: 'created' } },
						status: 201
					}
				: { data: { ping: { ok: true } }, status: 204 };
		}
	};

	const created = await executeCommand(client, CREATE, { id: 'one', title: 'created' });
	assert.deepEqual(created, {
		data: { id: 'one', title: 'created' },
		errors: undefined,
		status: 201
	});
	assert.deepEqual(calls[0].variables, {
		input: { id: 'one', title: 'created' }
	});

	const pinged = await executeCommand(client, PING);
	assert.deepEqual(pinged.data, { ok: true });
	assert.equal(pinged.status, 204);
	assert.equal(calls[1].variables, undefined);
});

test('bound commands use one direct API with or without a cache', async () => {
	const commands = defineCommands({ create: CREATE, ping: PING });
	const directCalls = [];
	const direct = bindCommands(
		{
			async request(document, variables) {
				directCalls.push({ document, variables });
				return String(document).includes('ping')
					? { data: { ping: { ok: true } }, status: 200 }
					: { data: { todos_create: { id: 'd' } }, status: 201 };
			}
		},
		commands
	);
	await direct.create({ id: 'd' });
	await direct.ping();
	assert.deepEqual(directCalls.map((call) => call.variables), [
		{ input: { id: 'd' } },
		undefined
	]);

	const cache = new QueryCache();
	const key = cacheKey(TODOS_DOCUMENT);
	cache.set(key, { data: { todos: [] }, updatedAt: 1 });
	const pipelineCalls = [];
	const pipelined = bindCommands(
		{
			cache,
			async request(document, variables) {
				pipelineCalls.push({ document, variables });
				return String(document).includes('ping')
					? { data: { ping: { ok: true } }, status: 206 }
					: {
							data: { todos_create: { id: 'p', title: 'pipeline' } },
							status: 202
						};
			}
		},
		commands,
		{
			policies: {
				create: { result: { kind: 'ack' }, reconcile: { kind: 'none' } }
			}
		}
	);
	const created = await pipelined.create(
		{ id: 'p', title: 'pipeline' },
		{
			browser: true,
			optimistic: {
				targets: [{ document: TODOS_DOCUMENT, at: 'todos', by: 'id' }],
				row: { id: 'p', title: 'pipeline' }
			}
		}
	);
	assert.equal(created.status, 202);
	assert.deepEqual(created.data, { id: 'p', title: 'pipeline' });
	assert.deepEqual(pipelineCalls[0].variables, {
		input: { id: 'p', title: 'pipeline' }
	});
	assert.equal(cache.get(key)?.pending, true);

	const pinged = await pipelined.ping({ browser: true });
	assert.equal(pinged.status, 206);
	assert.equal(pipelineCalls[1].variables, undefined);
});

test('pipeline transport failures return status 0 and roll back optimistic state', async () => {
	const cache = new QueryCache();
	const key = cacheKey(TODOS_DOCUMENT);
	cache.set(key, {
		data: { todos: [{ id: 'existing', title: 'safe' }] },
		updatedAt: 1
	});
	const command = bindCommands(
		{
			cache,
			async request() {
				throw new Error('network down');
			}
		},
		defineCommands({ create: CREATE })
	);

	const result = await command.create(
		{ id: 'ghost', title: 'temporary' },
		{
			browser: true,
			optimistic: {
				targets: [{ document: TODOS_DOCUMENT, at: 'todos', by: 'id' }],
				row: { id: 'ghost', title: 'temporary' }
			}
		}
	);
	assert.equal(result.status, 0);
	assert.match(result.errors?.[0]?.message ?? '', /network down/);
	assert.deepEqual(cache.get(key)?.data.todos, [{ id: 'existing', title: 'safe' }]);
});

test('pipeline work from a cleared cache generation cannot restore old data', async () => {
	const cache = new QueryCache();
	const key = cacheKey(TODOS_DOCUMENT);
	cache.set(key, {
		data: { todos: [{ id: 'alice', title: 'private' }] },
		updatedAt: 1
	});
	const response = deferredPipelineResult();
	let onErrorCalled = false;
	let settled = false;
	const command = runCommandPipeline(
		{
			cache,
			request: () => response.promise
		},
		CREATE_DOCUMENT,
		{ id: 'ghost', title: 'temporary' },
		{
			browser: true,
			optimistic: {
				targets: [{ document: TODOS_DOCUMENT, at: 'todos', by: 'id' }],
				row: { id: 'ghost', title: 'temporary' }
			},
			onError: () => {
				onErrorCalled = true;
				return [];
			},
			onSettled: () => {
				settled = true;
			}
		}
	);
	assert.deepEqual(cache.get(key)?.data.todos, [
		{ id: 'alice', title: 'private' },
		{ id: 'ghost', title: 'temporary' }
	]);

	cache.clear();
	cache.set(key, {
		data: { todos: [{ id: 'bob', title: 'current' }] },
		updatedAt: 2
	});
	response.resolve({ errors: [{ message: 'old request failed' }], status: 500 });
	await command;

	assert.deepEqual(cache.get(key)?.data.todos, [{ id: 'bob', title: 'current' }]);
	assert.equal(onErrorCalled, false);
	assert.equal(settled, true);
});

function deferredPipelineResult() {
	let resolve;
	const promise = new Promise((done) => {
		resolve = done;
	});
	return { promise, resolve };
}

test('runCommandPipeline omits variables for zero-input calls and preserves response status', async () => {
	let variables = 'not-called';
	const result = await runCommandPipeline(
		{
			cache: new QueryCache(),
			request: async (_document, seenVariables) => {
				variables = seenVariables;
				return { data: { ping: { ok: true } }, status: 299 };
			}
		},
		PING_DOCUMENT,
		undefined,
		{ browser: false }
	);
	assert.equal(variables, undefined);
	assert.equal(result.status, 299);
});

const MANIFEST = {
	version: 1,
	commands: [
		{
			command_name: 'Create a todo',
			field_name: 'todos_create',
			roles: ['user', 'admin'],
			input: {
				name: 'TodoInput',
				fields: [
					{ name: 'id', type_name: 'ID', nullable: false, list: false },
					{ name: 'tags', type_name: 'String', nullable: true, list: true }
				]
			},
			output: {
				name: 'Todo',
				fields: [
					{ name: 'id', type_name: 'ID', nullable: false, list: false },
					{ name: 'title', type_name: 'String', nullable: false, list: false }
				]
			},
			client_reconcile: {
				result: { kind: 'fact' },
				reconcile: { kind: 'subscription' }
			}
		},
		{
			command_name: 'Ping',
			field_name: 'ping',
			roles: [],
			output: {
				name: 'PingResult',
				fields: [{ name: 'ok', type_name: 'Boolean', nullable: false, list: false }]
			}
		}
	]
};

test('codegen validates once and produces deterministic aligned artifacts', () => {
	const normalized = parseCommandManifest(structuredClone(MANIFEST));
	const first = generateCommandArtifacts(normalized);
	const second = generateCommandArtifacts(structuredClone(MANIFEST));
	assert.deepEqual(first, second);
	assert.ok(first.commands.endsWith('\n'));
	assert.ok(first.operations.endsWith('\n'));
	assert.ok(first.policies.endsWith('\n'));
	assert.match(first.commands, /export const COMMANDS = defineCommands/);
	assert.match(first.commands, /export function todosCreate/);
	assert.match(first.commands, /hasInput: false/);
	assert.match(first.commands, /tags\?: Array<string> \| null/);
	assert.match(first.operations, /mutation Command_todos_create\(\$input: TodoInput!\)/);
	assert.match(first.operations, /mutation Command_ping \{/);
	assert.match(first.policies, /todosCreate/);
	assert.match(first.policies, /kind: "fact"/);
	assert.equal(fieldToFunctionName('todos_force_archive'), 'todosForceArchive');
	assert.ok(first.operations.includes(buildMutationOperation(normalized.commands[0])));
});

test('v3 codegen consumes exact causal operations and emits one shared protocol descriptor', () => {
	const commandOperation =
		'mutation Client_todos_create($commandId: ID!, $input: TodoInput!) { todos_create(commandId: $commandId, input: $input) { id title } }';
	const statusOperation =
		'query Distributed_CommandStatus($commandId: ID!) { commandStatus(commandId: $commandId) { state } }';
	const manifest = {
		manifest_version: 3,
		protocol_version: 2,
		schema_fingerprint: hash('role-selected-schema'),
		capabilities: { causal_receipts: true },
		commands: [
			{
				version: 1,
				name: 'todo.create',
				mutation_field: 'todos_create',
				grants: ['user'],
				input: {
					kind: 'object',
					definition: {
						name: 'TodoInput',
						fields: [
							{
								name: 'title',
								type_name: 'String',
								nullable: false,
								list: false,
								item_nullable: false,
								codec: 'string'
							}
						]
					}
				},
				output: {
					kind: 'object',
					definition: {
						name: 'Todo',
						fields: [
							{
								name: 'id',
								type_name: 'ID',
								nullable: false,
								list: false,
								item_nullable: false,
								codec: 'string'
							},
							{
								name: 'title',
								type_name: 'String',
								nullable: false,
								list: false,
								item_nullable: false,
								codec: 'string'
							}
						]
					}
				},
				operation: commandOperation,
				operation_hash: hash(commandOperation),
				extensions: {
					version: 2,
					consistency: { version: 1, kind: 'fact' },
					confirmations: {
						version: 1,
						kind: 'finite',
						expected: [],
						fallback: 'revalidate'
					}
				}
			}
		],
		protocol_operations: {
			version: 1,
			command_status: {
				name: 'Distributed_CommandStatus',
				operation: statusOperation,
				operation_hash: hash(statusOperation)
			}
		}
	};

	const parsed = parseCodegenManifest(manifest);
	const artifacts = generateCommandArtifacts(parsed);
	assert.ok(artifacts.operations.includes(commandOperation));
	assert.ok(artifacts.operations.includes(statusOperation));
	assert.ok(
		artifacts.commands.includes(
			`${JSON.stringify('todos_create')}: ${JSON.stringify(commandOperation)},`
		)
	);
	assert.ok(
		artifacts.commands.includes(`document: ${JSON.stringify(statusOperation)},`)
	);
	assert.match(artifacts.commands, /export const COMMAND_PROTOCOL = defineCausalProtocol/);
	assert.match(
		artifacts.commands,
		new RegExp(`operationHash: ${JSON.stringify(hash(commandOperation))}`)
	);
	assert.match(artifacts.commands, /projects: true/);
	assert.match(
		artifacts.commands,
		/export function todosCreate\(input: TodoInput, client: CommandClient, options\?: CommandCallOptions\)/
	);
	assert.equal(
		artifacts.operations.match(/Distributed_CommandStatus/g)?.length,
		1
	);
	const accepted = structuredClone(manifest);
	accepted.commands[0].extensions.consistency.kind = 'accepted';
	delete accepted.commands[0].extensions.confirmations;
	assert.match(generateCommandArtifacts(accepted).commands, /projects: false/);

	const drifted = structuredClone(manifest);
	drifted.commands[0].operation_hash = hash('different bytes');
	assert.throws(
		() => parseCodegenManifest(drifted),
		/does not match operation bytes/
	);

	const missingStatus = structuredClone(manifest);
	delete missingStatus.protocol_operations.command_status;
	assert.throws(
		() => parseCodegenManifest(missingStatus),
		/require command_status/
	);
});

test('codegen rejects malformed, ambiguous, and unsafe manifests', () => {
	assert.throws(
		() => parseCommandManifest({ version: 2, commands: [] }),
		/expected version 1/
	);
	assert.throws(
		() =>
			parseCommandManifest({
				version: 1,
				commands: [
					{ command_name: 'A', field_name: 'same', roles: [] },
					{ command_name: 'B', field_name: 'same', roles: [] }
				]
			}),
		/duplicate field same/
	);
	assert.throws(
		() =>
			parseCommandManifest({
				version: 1,
				commands: [{ command_name: 'Bad', field_name: 'not-valid', roles: [] }]
			}),
		/not a GraphQL name/
	);
	assert.throws(
		() =>
			parseCommandManifest({
				version: 1,
				commands: [{ command_name: 'Unsafe', field_name: 'delete', roles: [] }]
			}),
		/reserved TypeScript name delete/
	);
	assert.throws(
		() =>
			parseCommandManifest({
				version: 1,
				commands: [
					{
						command_name: 'Bad policy',
						field_name: 'bad_policy',
						roles: [],
						client_reconcile: { result: { kind: 'maybe' } }
					}
				]
			}),
		/unsupported result kind maybe/
	);
	assert.throws(
		() => generateCommandPoliciesTs(MANIFEST, { commandsImport: '@app/commands' }),
		/relative NodeNext specifier/
	);

	const nestedOutput = structuredClone(MANIFEST);
	nestedOutput.commands[0].output.fields[1].type_name = 'ChildPayload';
	assert.throws(
		() => generateCommandArtifacts(nestedOutput),
		/\.output\.fields\[1\]\.type_name "ChildPayload" is unsupported; manifest version 1 can only generate scalar fields/
	);

	const commentInjection = structuredClone(MANIFEST);
	commentInjection.commands[0].command_name = 'Create */ export const injected = true; /*';
	const generated = generateCommandArtifacts(commentInjection).commands;
	assert.doesNotMatch(generated, /Create \*\/ export const injected/);
	assert.match(generated, /Create \*\\\/ export const injected/);
});

test('command definitions validate field names and are frozen', () => {
	assert.throws(
		() => defineCommand({ field: '   ', document: PING_DOCUMENT, hasInput: false }),
		/must not be empty/
	);
	assert.equal(Object.isFrozen(CREATE), true);
	const commands = defineCommands({ create: CREATE });
	assert.equal(Object.isFrozen(commands), true);
});

function hash(value) {
	return `sha256:${createHash('sha256').update(value).digest('hex')}`;
}
