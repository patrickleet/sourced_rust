import assert from 'node:assert/strict';
import test from 'node:test';

import {
	createReplicaCommandRuntime,
	replicaCommandAuthority,
	ReplicaCommandRuntimeError
} from '../dist/replica/command-runtime.js';
import {
	createDistributedReplica,
	replicaRecordKey
} from '../dist/replica/index.js';

const HASH_A = `sha256:${'a'.repeat(64)}`;
const HASH_B = `sha256:${'b'.repeat(64)}`;
const HASH_C = `sha256:${'c'.repeat(64)}`;
const HASH_D = `sha256:${'d'.repeat(64)}`;
const COMMAND_A = '018f47de-3d2a-7abc-8abc-0123456789ab';
const COMMAND_B = '018f47de-3d2a-7def-8def-0123456789ab';
const GENERATED_ID = '018f47de-3d2a-7123-8123-0123456789ab';
const SURFACE = Object.freeze({ kind: 'role', name: 'user' });
const SCOPE = Object.freeze({
	protocolVersion: 2,
	schemaHash: HASH_B,
	cacheScope: 'scope:user'
});
const Todo = Object.freeze({ id: 'Todo', identityFields: Object.freeze(['id']) });
const STATUS_DOCUMENT =
	'query Distributed_CommandStatus($commandId: ID!) { commandStatus(commandId: $commandId) { state } }';
const COMMAND_STATUS = Object.freeze({
	name: 'Distributed_CommandStatus',
	document: STATUS_DOCUMENT,
	operationHash: HASH_D,
	protocol: Object.freeze({
		version: 2,
		schemaHash: HASH_B,
		protocolHash: HASH_C,
		surface: SURFACE,
		operation: HASH_D,
		trustedPresets: Object.freeze([
			Object.freeze({ name: 'owner', codec: 'string' })
		])
	})
});

const scalar = (name, typeName = 'String', codec = 'string', overrides = {}) =>
	Object.freeze({
		name,
		typeName,
		codec,
		nullable: false,
		list: false,
		itemNullable: false,
		...overrides
	});

const inputExpression = (path) =>
	Object.freeze({ kind: 'input', path: Object.freeze(path) });
const trustedPreset = (name) => Object.freeze({ kind: 'trusted_preset', name });
const effectField = (field, value) => Object.freeze({ field, value });
const effectKey = (...fields) => Object.freeze({ fields: Object.freeze(fields) });

const TodoInput = Object.freeze({
	name: 'TodoInput',
	fields: Object.freeze([scalar('id', 'ID'), scalar('title')])
});
const TodoOutput = Object.freeze({
	name: 'Todo',
	fields: Object.freeze([scalar('id', 'ID'), scalar('title')])
});
const CommandOutput = Object.freeze({
	name: 'CommandResult',
	fields: Object.freeze([scalar('ok', 'Boolean', 'boolean')])
});

function artifact(overrides = {}) {
	return Object.freeze({
		version: 1,
		name: 'todo.create',
		mutationField: 'createTodo',
		document:
			'mutation Client_createTodo($commandId: ID!, $input: TodoInput!) { createTodo(commandId: $commandId, input: $input) }',
		operationHash: HASH_A,
		protocol: Object.freeze({
			version: 2,
			schemaHash: HASH_B,
			protocolHash: HASH_C,
			surface: SURFACE,
			operation: HASH_A,
			trustedPresets: Object.freeze([
				Object.freeze({ name: 'owner', codec: 'string' })
			])
		}),
		input: Object.freeze({ kind: 'object', definition: TodoInput }),
		output: Object.freeze({ kind: 'object', definition: CommandOutput }),
		consistency: 'fact',
		effects: Object.freeze({
			version: 1,
			operations: Object.freeze([
				Object.freeze({
					kind: 'upsert',
					model: 'Todo',
					key: effectKey(effectField('id', inputExpression(['id']))),
					fields: Object.freeze([
						effectField('title', inputExpression(['title'])),
						effectField('owner', trustedPreset('owner'))
					])
				})
			]),
			fallback: 'revalidate'
		}),
		confirmations: Object.freeze({
			version: 1,
			kind: 'finite',
			expected: Object.freeze([
				Object.freeze({
					projector: 'todos',
					model: 'Todo',
					key: effectKey(effectField('id', inputExpression(['id'])))
				})
			]),
			fallback: 'revalidate'
		}),
		trustedPresets: Object.freeze([
			Object.freeze({ name: 'owner', codec: 'string' })
		]),
		revalidation: Object.freeze({
			version: 1,
			required: false,
			dependencies: Object.freeze(['todos']),
			models: Object.freeze(['Todo']),
			relationships: Object.freeze([])
		}),
		...overrides
	});
}

function commandEnvelope(commandId, options = {}) {
	const state = options.state ?? 'accepted_pending_projection';
	const consistency = options.consistency ?? 'fact';
	const expects = options.expects ?? [
		{ projection: 'todos', model: 'Todo', scopeToken: 'todo:scope' }
	];
	return {
		data:
			options.data === undefined
				? { createTodo: { ok: true } }
				: options.data,
		errors: options.errors,
		status: options.status ?? 200,
		extensions: {
			distributed: {
				protocolVersion: 2,
				schemaHash: HASH_B,
				cacheScope: 'scope:user',
				operation: options.operation ?? HASH_A,
				trustedPresets:
					options.trustedPresets ??
					[{ name: 'owner', codec: 'string', value: 'user-1' }],
				command: {
					commandId,
					causationId: options.causationId ?? `cause:${commandId}`,
					state,
					consistency,
					expects,
					observations: options.observations ?? [],
					records: options.records ?? []
				}
			}
		}
	};
}

function scopeQueryArtifact() {
	return Object.freeze({
		id: HASH_D,
		document: 'query ClientScope { todos { id title } }',
		protocol: Object.freeze({
			version: 2,
			schemaHash: HASH_B,
			surface: SURFACE,
			operation: HASH_D,
			trustedPresets: Object.freeze([
				Object.freeze({ name: 'owner', codec: 'string' })
			])
		}),
		variableCodec: Object.freeze({
			version: 2,
			limits: Object.freeze({
				maxDepth: 8,
				maxBoolWidth: 32,
				maxInList: 64
			}),
			variables: Object.freeze({}),
			inputs: Object.freeze({})
		}),
		roots: Object.freeze([
			Object.freeze({
				responseKey: 'todos',
				field: 'todos',
				cardinality: 'many',
				nullable: false,
				dependencies: Object.freeze(['todos']),
				selection: Object.freeze({
					typename: Todo.id,
					storage: Object.freeze({
						kind: 'normalized',
						model: Todo.id,
						identityFields: Todo.identityFields
					}),
					members: Object.freeze([
						Object.freeze({
							kind: 'scalar',
							responseKey: 'id',
							field: 'id',
							codec: 'ID',
							nullable: false
						}),
						Object.freeze({
							kind: 'scalar',
							responseKey: 'title',
							field: 'title',
							codec: 'String',
							nullable: false
						})
					])
				})
			})
		])
	});
}

function scopeSnapshotEnvelope(position, options = {}) {
	const resume = Object.freeze({
		projection: 'todos',
		position,
		token: `resume:${position}`
	});
	return {
		data: { todos: [] },
		extensions: {
			distributed: {
				protocolVersion: 2,
				schemaHash: HASH_B,
				cacheScope: 'scope:user',
				operation: HASH_D,
				trustedPresets: [
					{ name: 'owner', codec: 'string', value: 'user-1' }
				],
				...(options.command === undefined
					? {}
					: { command: options.command }),
				snapshot: {
					scopeToken: 'snapshot:scope',
					complete: true,
					records: [],
					indexes: [
						{
							projection: 'todos',
							scopeToken: 'index:todos',
							position,
							resume
						}
					],
					observations: options.observations ?? []
				}
			}
		}
	};
}

function statusEnvelope(commandId, options = {}) {
	const state = options.state ?? 'accepted_pending_projection';
	const envelope = commandEnvelope(commandId, {
		...options,
		state,
		operation: options.operation ?? HASH_D,
		data: { commandStatus: { state } }
	});
	if (options.omitMetadata) {
		delete envelope.extensions.distributed.command;
	}
	return envelope;
}

class FakeReplica {
	scope = SCOPE;
	authorizationGeneration = 1;
	base = new Map();
	layers = new Map();
	accepted = new Map();
	contracts = [];
	authority = {
		generation: 1,
		scope: SCOPE,
		trustedPresets: Object.freeze([
			Object.freeze({ name: 'owner', codec: 'string', value: 'user-1' })
		]),
		controller: new AbortController()
	};

	[replicaCommandAuthority] = (contract) => {
		this.contracts.push(contract);
		return Object.freeze({
			read: () => ({
				generation: this.authority.generation,
				scope: this.authority.scope,
				trustedPresets: this.authority.trustedPresets,
				signal: this.authority.controller.signal
			}),
			dispose() {}
		});
	};

	createOptimisticLayer(id, update, semanticChanges = []) {
		if (this.layers.has(id)) throw new Error('optimistic layer already exists');
		const operations = [];
		update({
			writeRecord: (model, identity, patch) =>
				operations.push({
					kind: 'write',
					key: replicaRecordKey(model, identity),
					fields: patch.fields ?? {}
				}),
			tombstoneRecord: (model, identity) =>
				operations.push({
					kind: 'delete',
					key: replicaRecordKey(model, identity)
				}),
			writeIndex() {},
			deleteIndex() {}
		});
		this.layers.set(id, {
			operations,
			semanticChanges,
			state: 'optimistic'
		});
	}

	markOptimisticLayerAccepted(id, receipt) {
		const layer = this.layers.get(id);
		if (!layer) return false;
		layer.state = 'accepted';
		this.accepted.set(id, receipt);
		return true;
	}

	rejectOptimisticLayer(id) {
		this.accepted.delete(id);
		return this.layers.delete(id);
	}

	confirmOptimisticLayer(id, update) {
		const result = update({
			writeRecord: (model, identity, revision, patch) => {
				const key = replicaRecordKey(model, identity);
				this.base.set(key, {
					revision: String(revision),
					incarnation: String(patch.incarnation ?? revision),
					fields: { ...(this.base.get(key)?.fields ?? {}), ...(patch.fields ?? {}) }
				});
				return true;
			},
			tombstoneRecord: () => true,
			writeIndex: () => true,
			deleteIndex: () => true
		});
		this.layers.delete(id);
		this.accepted.delete(id);
		return result;
	}

	visible(model, identity) {
		const key = replicaRecordKey(model, identity);
		let record = this.base.get(key);
		let value =
			record === undefined
				? undefined
				: { revision: record.revision, fields: { ...record.fields } };
		for (const layer of this.layers.values()) {
			for (const operation of layer.operations) {
				if (operation.key !== key) continue;
				if (operation.kind === 'delete') value = undefined;
				else {
					value = {
						revision: 'optimistic',
						fields: { ...(value?.fields ?? {}), ...operation.fields }
					};
				}
			}
		}
		return value;
	}

	invalidateAuthorization() {
		this.authorizationGeneration += 1;
		this.scope = undefined;
		this.layers.clear();
	}

	// Unused DistributedReplica methods keep this test double honest at the seam.
	read() {
		throw new Error('unused');
	}
	watch() {
		throw new Error('unused');
	}
	writeResult() {
		throw new Error('unused');
	}
	dehydrate() {
		throw new Error('unused');
	}
	hydrate() {
		throw new Error('unused');
	}
	tombstoneRecord() {
		throw new Error('unused');
	}
	markIndexStale() {
		throw new Error('unused');
	}
	retainRecord() {}
	releaseRecord() {}
	gc() {
		return [];
	}
	inspectRecord() {
		return undefined;
	}
	inspectIndex() {
		return undefined;
	}
}

test('real replica authority gates commands on its server-issued scope inventory', async () => {
	const replica = createDistributedReplica();
	const command = artifact();
	const scopeOperation = Object.freeze({
		id: HASH_D,
		document: 'query ClientScope { __typename }',
		protocol: Object.freeze({
			version: 2,
			schemaHash: HASH_B,
			surface: SURFACE,
			operation: HASH_D,
			trustedPresets: Object.freeze([
				Object.freeze({ name: 'owner', codec: 'string' })
			])
		}),
		variableCodec: Object.freeze({
			version: 2,
			limits: Object.freeze({
				maxDepth: 8,
				maxBoolWidth: 32,
				maxInList: 64
			}),
			variables: Object.freeze({}),
			inputs: Object.freeze({})
		}),
		roots: Object.freeze([
			Object.freeze({
				responseKey: 'todos',
				field: 'todos',
				cardinality: 'many',
				nullable: false,
				dependencies: Object.freeze(['todos']),
				selection: Object.freeze({
					typename: Todo.id,
					storage: Object.freeze({
						kind: 'normalized',
						model: Todo.id,
						identityFields: Todo.identityFields
					}),
					members: Object.freeze([
						Object.freeze({
							kind: 'scalar',
							responseKey: 'id',
							field: 'id',
							codec: 'ID',
							nullable: false
						}),
						Object.freeze({
							kind: 'scalar',
							responseKey: 'title',
							field: 'title',
							codec: 'String',
							nullable: false
						})
					])
				})
			})
		])
	});
	const runtime = createReplicaCommandRuntime(
		replica,
		{
			dispatch: (request) =>
				Promise.resolve(
					commandEnvelope(request.commandId, {
						observations: [
							{
								causationId: `cause:${request.commandId}`,
								projection: 'todos',
								model: 'Todo',
								scopeToken: 'todo:scope'
							}
						]
					})
				)
		},
		{ createTodo: command }
	);

	await assert.rejects(
		runtime.commands.createTodo(
			{ id: 'todo-before-scope', title: 'blocked' },
			{ commandId: COMMAND_A }
		),
		(error) =>
			error instanceof ReplicaCommandRuntimeError &&
			error.code === 'REPLICA_COMMAND_AUTHORITY_UNAVAILABLE'
	);
	replica.writeResult(
		scopeOperation,
		{},
		{
			extensions: {
				distributed: {
					protocolVersion: 2,
					schemaHash: HASH_B,
					cacheScope: 'scope:user',
					operation: HASH_D,
					trustedPresets: [
						{ name: 'owner', codec: 'string', value: 'user-1' }
					]
				}
			}
		},
		'network'
	);

	const receipt = await runtime.commands.createTodo(
		{ id: 'todo-after-scope', title: 'allowed' },
		{ commandId: COMMAND_B }
	);
	assert.ok(replica.inspectRecord(Todo, 'todo-after-scope'));
	replica.writeResult(
		scopeOperation,
		{},
		{
			data: { todos: [] },
			extensions: {
				distributed: {
					protocolVersion: 2,
					schemaHash: HASH_B,
					cacheScope: 'scope:user',
					operation: HASH_D,
					trustedPresets: [
						{ name: 'owner', codec: 'string', value: 'user-1' }
					],
					command: receipt.metadata,
					snapshot: {
						scopeToken: 'snapshot:scope',
						complete: true,
						records: [],
						indexes: [],
						observations: [
							{
								causationId: receipt.metadata.causationId,
								projection: 'todos',
								model: 'Todo',
								scopeToken: 'todo:scope'
							}
						]
					}
				}
			}
		},
		'network'
	);
	assert.equal((await receipt.projected).state, 'projected');
	assert.equal(replica.inspectRecord(Todo, 'todo-after-scope'), undefined);
	runtime.dispose();
});

test('lower snapshot from the active source retires causally satisfied optimism and resolves projected', async () => {
	const replica = createDistributedReplica();
	const command = artifact();
	const scopeOperation = scopeQueryArtifact();
	const runtime = createReplicaCommandRuntime(
		replica,
		{
			dispatch: (request) =>
				Promise.resolve(commandEnvelope(request.commandId))
		},
		{ createTodo: command }
	);

	replica.writeResult(
		scopeOperation,
		{},
		scopeSnapshotEnvelope('5'),
		'network'
	);
	const receipt = await runtime.commands.createTodo(
		{ id: 'todo-lower', title: 'pending' },
		{ commandId: COMMAND_B }
	);
	let projected;
	void receipt.projected.then((outcome) => {
		projected = outcome;
	});

	replica.writeResult(
		scopeOperation,
		{},
		scopeSnapshotEnvelope('4', {
			command: receipt.metadata,
			observations: [
				{
					causationId: receipt.metadata.causationId,
					projection: 'todos',
					model: 'Todo',
					scopeToken: 'todo:scope'
				}
			]
		}),
		'network'
	);
	await Promise.resolve();
	assert.equal(projected?.state, 'projected');

	// The transaction retired the old layer even though its index position was
	// lower; the same identity can be used for a fresh test layer.
	replica.createOptimisticLayer(COMMAND_B, () => undefined);
	assert.equal(replica.rejectOptimisticLayer(COMMAND_B), true);
	runtime.dispose();
});

test('binder registers the exact preset union and retries one frozen prepared unit', async () => {
	const replica = new FakeReplica();
	let uuidCalls = 0;
	const requests = [];
	let acceptedEnvelope;
	const transport = {
		async dispatch(request) {
			requests.push(request);
			assert.equal(replica.visible(Todo, GENERATED_ID).fields.owner, 'user-1');
			if (requests.length === 1) throw new Error('ambiguous disconnect');
			acceptedEnvelope = commandEnvelope(request.commandId, {
				observations: [
					{
						causationId: `cause:${request.commandId}`,
						projection: 'todos',
						model: 'Todo',
						scopeToken: 'todo:scope'
					}
				]
			});
			return acceptedEnvelope;
		}
	};
	const command = artifact({
		inputDefaults: Object.freeze({
			version: 1,
			defaults: Object.freeze([
				Object.freeze({ path: Object.freeze(['id']), generator: 'uuid_v7' })
			])
		})
	});
	const runtime = createReplicaCommandRuntime(replica, transport, {
		createTodo: command
	});

	const receipt = await runtime.commands.createTodo(
		{ title: 'one' },
		{
			commandId: COMMAND_A,
			transportRetries: 1,
			generators: {
				uuidV7: () => {
					uuidCalls += 1;
					return GENERATED_ID;
				}
			}
		}
	);

	assert.equal(uuidCalls, 1);
	assert.equal(requests.length, 2);
	assert.equal(requests[0], requests[1], 'retry must reuse the exact request object');
	assert.equal(requests[0].variables.input.id, GENERATED_ID);
	assert.deepEqual(requests[0].extensions, {
		distributed: {
			client: {
				surface: SURFACE,
				schemaHash: HASH_B
			}
		}
	});
	assert.deepEqual(replica.contracts[0].trustedPresets, [
		{ name: 'owner', codec: 'string' }
	]);
	assert.equal(replica.layers.get(COMMAND_A).state, 'accepted');
	assert.equal(replica.layers.get(COMMAND_A).semanticChanges.length, 0);
	assert.deepEqual(receipt.result, { ok: true });
	assert.ok(Object.isFrozen(receipt.result));
	let projectedSettled = false;
	void receipt.projected.then(() => {
		projectedSettled = true;
	});
	await Promise.resolve();
	assert.equal(
		projectedSettled,
		false,
		'receipt observations cannot confirm canonical cache ingestion'
	);
	replica.confirmOptimisticLayer(COMMAND_A, () => undefined);
	runtime.observeResult(acceptedEnvelope);
	assert.deepEqual(await receipt.projected, {
		commandId: COMMAND_A,
		state: 'projected',
		metadata: receipt.metadata
	});
	runtime.dispose();
});

test('pre-dispatch validation and an already-aborted caller never create optimism', async () => {
	const replica = new FakeReplica();
	let dispatches = 0;
	let statusReads = 0;
	const runtime = createReplicaCommandRuntime(
		replica,
		{
			dispatch(request) {
				dispatches += 1;
				return Promise.resolve(commandEnvelope(request.commandId));
			},
			status(request) {
				statusReads += 1;
				return Promise.resolve(statusEnvelope(request.commandId));
			}
		},
		{ createTodo: artifact() },
		{ status: COMMAND_STATUS }
	);

	await assert.rejects(
		runtime.commands.createTodo(
			{ id: 'todo-invalid-retries', title: 'must-not-appear' },
			{ commandId: COMMAND_A, transportRetries: -1 }
		),
		/transportRetries must be an integer/
	);
	assert.equal(replica.layers.size, 0);

	const caller = new AbortController();
	caller.abort('deadline elapsed before invocation');
	await assert.rejects(
		runtime.commands.createTodo(
			{ id: 'todo-already-aborted', title: 'must-not-appear' },
			{ commandId: COMMAND_B, signal: caller.signal }
		),
		(error) =>
			error instanceof ReplicaCommandRuntimeError &&
			error.code === 'REPLICA_COMMAND_ABORTED' &&
			error.recovery === undefined
	);
	assert.equal(replica.layers.size, 0);
	assert.equal(dispatches, 0);
	assert.equal(statusReads, 0);
	runtime.dispose();
});

test('same-scope hydration replaces confirmed state without invalidating accepted optimism', async () => {
	const replica = createDistributedReplica();
	const navigationReplica = createDistributedReplica();
	const command = artifact();
	const scopeOperation = scopeQueryArtifact();
	const runtime = createReplicaCommandRuntime(
		replica,
		{
			dispatch: (request) =>
				Promise.resolve(commandEnvelope(request.commandId))
		},
		{ createTodo: command }
	);

	replica.writeResult(
		scopeOperation,
		{},
		scopeSnapshotEnvelope('1'),
		'network'
	);
	const generation = replica.authorizationGeneration;
	const receipt = await runtime.commands.createTodo(
		{ id: 'todo-navigation', title: 'optimistic across navigation' },
		{ commandId: COMMAND_A }
	);
	assert.ok(replica.inspectRecord(Todo, 'todo-navigation'));

	navigationReplica.read(scopeOperation, {});
	navigationReplica.writeResult(
		scopeOperation,
		{},
		scopeSnapshotEnvelope('2'),
		'network'
	);
	const navigationState = navigationReplica.dehydrate();
	assert.equal(replica.hydrate(navigationState, SCOPE), true);
	assert.equal(
		replica.authorizationGeneration,
		generation,
		'same-scope hydration must not close command authority'
	);
	assert.ok(
		replica.inspectRecord(Todo, 'todo-navigation'),
		'accepted optimism must remain above the replacement confirmed base'
	);

	replica.writeResult(
		scopeOperation,
		{},
		scopeSnapshotEnvelope('3', {
			command: receipt.metadata,
			observations: [
				{
					causationId: receipt.metadata.causationId,
					projection: 'todos',
					model: 'Todo',
					scopeToken: 'todo:scope'
				}
			]
		}),
		'network'
	);
	assert.equal((await receipt.projected).state, 'projected');
	assert.equal(replica.inspectRecord(Todo, 'todo-navigation'), undefined);
	runtime.dispose();
});

test('explicit rejection removes only its own layer and rebases later work', async () => {
	const replica = new FakeReplica();
	replica.base.set(replicaRecordKey(Todo, 'todo-1'), {
		revision: '1',
		incarnation: '1',
		fields: { id: 'todo-1', title: 'base' }
	});
	let resolveDispatch;
	const transport = {
		dispatch: () =>
			new Promise((resolve) => {
				resolveDispatch = resolve;
			})
	};
	const acceptedArtifact = artifact({
		name: 'todo.rename',
		mutationField: 'renameTodo',
		document:
			'mutation Client_renameTodo($commandId: ID!, $input: TodoInput!) { renameTodo(commandId: $commandId, input: $input) }',
		consistency: 'accepted',
		trustedPresets: Object.freeze([]),
		confirmations: undefined,
		effects: {
			version: 1,
			operations: [
				{
					kind: 'patch',
					model: 'Todo',
					key: effectKey(effectField('id', inputExpression(['id']))),
					fields: [
						effectField('title', inputExpression(['title']))
					]
				}
			],
			fallback: 'revalidate'
		}
	});
	const runtime = createReplicaCommandRuntime(replica, transport, {
		renameTodo: acceptedArtifact
	});
	const rejected = runtime.commands.renameTodo(
		{ id: 'todo-1', title: 'A' },
		{ commandId: COMMAND_A }
	);
	await Promise.resolve();
	assert.equal(replica.visible(Todo, 'todo-1').fields.title, 'A');

	replica.createOptimisticLayer(COMMAND_B, (writer) => {
		writer.writeRecord(Todo, 'todo-1', { fields: { title: 'B' } });
	});
	resolveDispatch(
		commandEnvelope(COMMAND_A, {
			state: 'rejected',
			consistency: 'accepted',
			expects: [],
			operation: HASH_A,
			data: null,
			errors: [{ message: 'denied', extensions: { code: 'REJECTED' } }]
		})
	);

	await assert.rejects(
		rejected,
		(error) =>
			error instanceof ReplicaCommandRuntimeError &&
			error.code === 'REPLICA_COMMAND_REJECTED'
	);
	assert.equal(replica.layers.has(COMMAND_A), false);
	assert.equal(replica.layers.has(COMMAND_B), true);
	assert.equal(replica.visible(Todo, 'todo-1').fields.title, 'B');
	runtime.dispose();
});

test('Projected<T> validates and writes canonical data while retiring its layer atomically', async () => {
	const replica = new FakeReplica();
	replica.base.set(replicaRecordKey(Todo, 'todo-1'), {
		revision: '1',
		incarnation: '1',
		fields: { id: 'todo-1', title: 'base' }
	});
	const projectedArtifact = artifact({
		name: 'todo.project',
		mutationField: 'projectTodo',
		document:
			'mutation Client_projectTodo($commandId: ID!, $input: TodoInput!) { projectTodo(commandId: $commandId, input: $input) { id title } }',
		output: Object.freeze({ kind: 'object', definition: TodoOutput }),
		consistency: 'projected',
		trustedPresets: Object.freeze([]),
		confirmations: undefined,
		effects: {
			version: 1,
			operations: [
				{
					kind: 'patch',
					model: 'Todo',
					key: effectKey(effectField('id', inputExpression(['id']))),
					fields: [
						effectField('title', inputExpression(['title']))
					]
				}
			],
			fallback: 'revalidate'
		},
		directProjection: Object.freeze({
			topology: Object.freeze({
				version: 1,
				name: 'todos',
				digest: HASH_C
			}),
			model: 'Todo',
			identityFields: Object.freeze(['id']),
			changeEpoch: 'todos-v1'
		})
	});
	const transport = {
		async dispatch(request) {
			assert.equal(replica.visible(Todo, 'todo-1').fields.title, 'optimistic');
			return commandEnvelope(request.commandId, {
				state: 'projected',
				consistency: 'projected',
				expects: [],
				data: { projectTodo: { id: 'todo-1', title: 'canonical' } },
				records: [
					{
						model: 'Todo',
						scopeToken: 'record:todo-1',
						incarnation: '1',
						revision: '2',
						tombstone: false
					}
				]
			});
		}
	};
	const runtime = createReplicaCommandRuntime(replica, transport, {
		projectTodo: projectedArtifact
	});

	const receipt = await runtime.commands.projectTodo(
		{ id: 'todo-1', title: 'optimistic' },
		{ commandId: COMMAND_A }
	);

	assert.equal(replica.layers.has(COMMAND_A), false);
	assert.deepEqual(replica.visible(Todo, 'todo-1'), {
		revision: '2',
		fields: { id: 'todo-1', title: 'canonical' }
	});
	assert.deepEqual(await receipt.projected, {
		commandId: COMMAND_A,
		state: 'projected',
		result: { id: 'todo-1', title: 'canonical' },
		metadata: receipt.metadata
	});
	runtime.dispose();
});

test('relationship and invalidation effects stay semantic instead of guessing list links', async () => {
	const replica = new FakeReplica();
	const relationshipArtifact = artifact({
		name: 'todo.relate',
		mutationField: 'relateTodo',
		document:
			'mutation Client_relateTodo($commandId: ID!, $input: TodoInput!) { relateTodo(commandId: $commandId, input: $input) }',
		consistency: 'accepted',
		trustedPresets: Object.freeze([]),
		confirmations: undefined,
		effects: {
			version: 1,
			operations: [
				{
					kind: 'link',
					relationship: {
						sourceModel: 'Todo',
						field: 'related',
						targetModel: 'Todo'
					},
					source: effectKey(
						effectField('id', inputExpression(['id']))
					),
					target: effectKey(
						effectField('id', inputExpression(['title']))
					)
				},
				{ kind: 'invalidate_model', model: 'Todo' }
			],
			fallback: 'revalidate'
		}
	});
	const transport = {
		dispatch: (request) =>
			Promise.resolve(
				commandEnvelope(request.commandId, {
					state: 'accepted',
					consistency: 'accepted',
					expects: [],
					data: { relateTodo: { ok: true } }
				})
			)
	};
	const runtime = createReplicaCommandRuntime(replica, transport, {
		relateTodo: relationshipArtifact
	});

	await runtime.commands.relateTodo(
		{ id: 'todo-1', title: 'todo-2' },
		{ commandId: COMMAND_A }
	);

	const layer = replica.layers.get(COMMAND_A);
	assert.deepEqual(layer.operations, []);
	assert.deepEqual(layer.semanticChanges, [
		{
			kind: 'link',
			sourceModel: 'Todo',
			field: 'related',
			targetModel: 'Todo',
			sourceKey: replicaRecordKey(Todo, 'todo-1'),
			targetKey: replicaRecordKey(Todo, 'todo-2'),
			dependencies: ['todos']
		},
		{ kind: 'invalidate', dependencies: ['todos'] }
	]);
	runtime.dispose();
});

test('authorization abort rejects and untracks a pending projected awaitable', async () => {
	const replica = new FakeReplica();
	const transport = {
		dispatch: (request) => Promise.resolve(commandEnvelope(request.commandId))
	};
	const runtime = createReplicaCommandRuntime(replica, transport, {
		createTodo: artifact()
	});
	const receipt = await runtime.commands.createTodo(
		{ id: 'todo-1', title: 'pending' },
		{ commandId: COMMAND_A }
	);

	replica.authority.generation += 1;
	replica.authority.controller.abort('logout');
	await assert.rejects(
		receipt.projected,
		(error) =>
			error instanceof ReplicaCommandRuntimeError &&
			error.code === 'REPLICA_COMMAND_SCOPE_INVALIDATED'
	);

	// Late matching evidence cannot resurrect or re-resolve the settled handle.
	runtime.observeResult(
		commandEnvelope(COMMAND_A, {
			observations: [
				{
					causationId: `cause:${COMMAND_A}`,
					projection: 'todos',
					model: 'Todo',
					scopeToken: 'todo:scope'
				}
			]
		})
	);
	runtime.dispose();
});

test('caller abort after acceptance only rejects its projection wait while causality continues', async () => {
	const replica = createDistributedReplica();
	const command = artifact();
	const scopeOperation = scopeQueryArtifact();
	const caller = new AbortController();
	const statusRequests = [];
	const runtime = createReplicaCommandRuntime(
		replica,
		{
			dispatch: (request) =>
				Promise.resolve(commandEnvelope(request.commandId)),
			status(request) {
				statusRequests.push(request);
				return Promise.resolve(statusEnvelope(request.commandId));
			}
		},
		{ createTodo: command },
		{ status: COMMAND_STATUS }
	);
	replica.writeResult(
		scopeOperation,
		{},
		scopeSnapshotEnvelope('1'),
		'network'
	);
	const receipt = await runtime.commands.createTodo(
		{ id: 'todo-deadline', title: 'optimistic' },
		{ commandId: COMMAND_A, signal: caller.signal }
	);

	caller.abort('caller deadline');
	await assert.rejects(
		receipt.projected,
		(error) =>
			error instanceof ReplicaCommandRuntimeError &&
			error.code === 'REPLICA_COMMAND_ABORTED'
	);
	assert.ok(
		replica.inspectRecord(Todo, 'todo-deadline'),
		'caller cancellation must preserve the accepted optimistic layer'
	);

	const status = await receipt.status();
	assert.equal(status.state, 'accepted_pending_projection');
	assert.equal(statusRequests.length, 1);
	assert.notEqual(
		statusRequests[0].signal,
		caller.signal,
		'status recovery must remain bound to authority, not caller cancellation'
	);
	assert.equal(statusRequests[0].signal?.aborted, false);

	replica.writeResult(
		scopeOperation,
		{},
		scopeSnapshotEnvelope('2', {
			command: receipt.metadata,
			observations: [
				{
					causationId: receipt.metadata.causationId,
					projection: 'todos',
					model: 'Todo',
					scopeToken: 'todo:scope'
				}
			]
		}),
		'network'
	);
	assert.equal(
		replica.inspectRecord(Todo, 'todo-deadline'),
		undefined,
		'later causal evidence must still retire the optimistic layer'
	);
	await assert.rejects(
		receipt.projected,
		(error) =>
			error instanceof ReplicaCommandRuntimeError &&
			error.code === 'REPLICA_COMMAND_ABORTED'
	);
	runtime.dispose();
});

test('matching wire observations resolve only after the replica retires the layer', async () => {
	const replica = new FakeReplica();
	const transport = {
		dispatch: (request) => Promise.resolve(commandEnvelope(request.commandId))
	};
	const runtime = createReplicaCommandRuntime(replica, transport, {
		createTodo: artifact()
	});
	const receipt = await runtime.commands.createTodo(
		{ id: 'todo-1', title: 'pending' },
		{ commandId: COMMAND_A }
	);
	const observed = commandEnvelope(COMMAND_A, {
		observations: [
			{
				causationId: `cause:${COMMAND_A}`,
				projection: 'todos',
				model: 'Todo',
				scopeToken: 'todo:scope'
			}
		]
	});
	let settled = false;
	void receipt.projected.then(() => {
		settled = true;
	});

	// Calling the hook early cannot turn a merely syntactic match into proof.
	runtime.observeResult(observed);
	await Promise.resolve();
	assert.equal(settled, false);
	assert.equal(replica.layers.has(COMMAND_A), true);

	// The query/live coordinator atomically merges base and retires the layer.
	replica.confirmOptimisticLayer(COMMAND_A, () => undefined);
	runtime.observeResult(observed);
	assert.deepEqual(await receipt.projected, {
		commandId: COMMAND_A,
		state: 'projected',
		metadata: observed.extensions.distributed.command
	});
	runtime.dispose();
});

test('generated status is required at construction and coalesces exact causal reads', async () => {
	const missingStatusReplica = new FakeReplica();
	assert.throws(
		() =>
			createReplicaCommandRuntime(
				missingStatusReplica,
				{ dispatch: () => Promise.reject(new Error('unused')) },
				{ createTodo: artifact() },
				{ status: COMMAND_STATUS }
			),
		/generated command status artifact requires transport\.status/
	);

	const replica = new FakeReplica();
	const statusRequests = [];
	const revalidations = [];
	let resolveStatus;
	const transport = {
		dispatch: (request) => Promise.resolve(commandEnvelope(request.commandId)),
		status(request) {
			statusRequests.push(request);
			return new Promise((resolve) => {
				resolveStatus = resolve;
			});
		},
		revalidate(request) {
			revalidations.push(request);
		}
	};
	const runtime = createReplicaCommandRuntime(
		replica,
		transport,
		{ createTodo: artifact() },
		{ status: COMMAND_STATUS }
	);
	const receipt = await runtime.commands.createTodo(
		{ id: 'todo-status', title: 'pending' },
		{ commandId: COMMAND_A }
	);
	const projectedEnvelope = statusEnvelope(COMMAND_A, {
		state: 'projected',
		observations: [
			{
				causationId: `cause:${COMMAND_A}`,
				projection: 'todos',
				model: 'Todo',
				scopeToken: 'todo:scope'
			}
		]
	});

	const first = receipt.status();
	const second = receipt.status();
	assert.equal(first, second, 'concurrent status reads must coalesce');
	assert.equal(statusRequests.length, 1);
	const { signal, ...wireRequest } = statusRequests[0];
	assert.equal(signal, replica.authority.controller.signal);
	assert.deepEqual(wireRequest, {
		operation: 'status',
		commandId: COMMAND_A,
		name: 'Distributed_CommandStatus',
		document: STATUS_DOCUMENT,
		operationHash: HASH_D,
		variables: { commandId: COMMAND_A },
		extensions: {
			distributed: {
				client: {
					surface: SURFACE,
					schemaHash: HASH_B
				}
			}
		}
	});
	resolveStatus(projectedEnvelope);
	const status = await first;
	assert.equal(status.state, 'projected');
	assert.equal(status.metadata.commandId, COMMAND_A);

	let projectedSettled = false;
	void receipt.projected.then(() => {
		projectedSettled = true;
	});
	await Promise.resolve();
	assert.equal(projectedSettled, false);
	assert.equal(replica.layers.has(COMMAND_A), true);
	assert.deepEqual(
		revalidations.map(({ reason }) => reason),
		['accepted-fallback']
	);

	replica.confirmOptimisticLayer(COMMAND_A, () => undefined);
	runtime.observeResult(projectedEnvelope);
	assert.equal((await receipt.projected).state, 'projected');
	runtime.dispose();
});

test('dotted generated names become frozen nested namespaces and reject prefix collisions', () => {
	const replica = new FakeReplica();
	const runtime = createReplicaCommandRuntime(
		replica,
		{ dispatch: () => Promise.reject(new Error('unused')) },
		{ 'todo.create': artifact() }
	);
	assert.equal(typeof runtime.commands.todo.create, 'function');
	assert.equal(runtime.commands['todo.create'], undefined);
	assert.equal(Object.isFrozen(runtime.commands), true);
	assert.equal(Object.isFrozen(runtime.commands.todo), true);
	runtime.dispose();

	assert.throws(
		() =>
			createReplicaCommandRuntime(
				new FakeReplica(),
				{ dispatch: () => Promise.reject(new Error('unused')) },
				{
					todo: artifact({ name: 'todo.root' }),
					'todo.create': artifact({ name: 'todo.create' })
				}
			),
		/replica command namespace collision at todo\.create/
	);
});

test('ambiguous and in-progress dispatches expose recovery without retiring optimism', async () => {
	const replica = new FakeReplica();
	const transport = {
		dispatch(request) {
			if (request.commandId === COMMAND_A) {
				return Promise.reject(new Error('response lost'));
			}
			return Promise.resolve(
				commandEnvelope(request.commandId, {
					state: 'in_progress',
					expects: []
				})
			);
		},
		status(request) {
			return Promise.resolve(
				request.commandId === COMMAND_A
					? statusEnvelope(COMMAND_A, {
							state: 'unknown',
							omitMetadata: true
						})
					: statusEnvelope(COMMAND_B)
			);
		}
	};
	const runtime = createReplicaCommandRuntime(
		replica,
		transport,
		{ createTodo: artifact() },
		{ status: COMMAND_STATUS }
	);

	let ambiguous;
	try {
		await runtime.commands.createTodo(
			{ id: 'todo-ambiguous', title: 'ambiguous' },
			{ commandId: COMMAND_A }
		);
		assert.fail('ambiguous dispatch should reject');
	} catch (error) {
		ambiguous = error;
	}
	assert.equal(ambiguous.code, 'REPLICA_COMMAND_TRANSPORT_AMBIGUOUS');
	assert.equal(ambiguous.recovery.commandId, COMMAND_A);
	assert.equal(replica.layers.get(COMMAND_A).state, 'optimistic');
	const unknown = await ambiguous.recovery.status();
	assert.deepEqual(unknown, { commandId: COMMAND_A, state: 'unknown' });
	assert.equal(replica.layers.get(COMMAND_A).state, 'optimistic');

	let inProgress;
	try {
		await runtime.commands.createTodo(
			{ id: 'todo-progress', title: 'progress' },
			{ commandId: COMMAND_B }
		);
		assert.fail('in-progress dispatch should reject');
	} catch (error) {
		inProgress = error;
	}
	assert.equal(inProgress.code, 'REPLICA_COMMAND_OUTCOME_PENDING');
	assert.equal(inProgress.recovery.commandId, COMMAND_B);
	assert.equal(replica.layers.get(COMMAND_B).state, 'optimistic');
	assert.equal((await inProgress.recovery.status()).state, 'accepted_pending_projection');
	assert.equal(replica.layers.get(COMMAND_B).state, 'accepted');

	replica.rejectOptimisticLayer(COMMAND_A);
	replica.rejectOptimisticLayer(COMMAND_B);
	runtime.dispose();
});

test('terminal status rolls back only its layer and rejects its projected awaitable', async () => {
	const replica = new FakeReplica();
	const revalidations = [];
	const transport = {
		dispatch: (request) => Promise.resolve(commandEnvelope(request.commandId)),
		status: (request) =>
			Promise.resolve(
				statusEnvelope(request.commandId, {
					state: 'projection_failed'
				})
			),
		revalidate(request) {
			revalidations.push(request);
		}
	};
	const runtime = createReplicaCommandRuntime(
		replica,
		transport,
		{ createTodo: artifact() },
		{ status: COMMAND_STATUS }
	);
	const receipt = await runtime.commands.createTodo(
		{ id: 'todo-failed', title: 'failed' },
		{ commandId: COMMAND_A }
	);
	replica.createOptimisticLayer(COMMAND_B, () => undefined);
	const projectedFailure = assert.rejects(
		receipt.projected,
		(error) =>
			error instanceof ReplicaCommandRuntimeError &&
			error.code === 'REPLICA_COMMAND_PROJECTION_FAILED'
	);

	assert.equal((await receipt.status()).state, 'projection_failed');
	await projectedFailure;
	assert.equal(replica.layers.has(COMMAND_A), false);
	assert.equal(replica.layers.has(COMMAND_B), true);
	assert.deepEqual(
		revalidations.map(({ reason }) => reason),
		['projection-failed']
	);
	replica.rejectOptimisticLayer(COMMAND_B);
	runtime.dispose();
});

test('post-dispatch protocol failure retains a reachable generated recovery handle', async () => {
	const replica = new FakeReplica();
	const transport = {
		dispatch: (request) =>
			Promise.resolve(
				commandEnvelope(request.commandId, {
					data: { unexpected: true }
				})
			),
		status: (request) => Promise.resolve(statusEnvelope(request.commandId))
	};
	const runtime = createReplicaCommandRuntime(
		replica,
		transport,
		{ createTodo: artifact() },
		{ status: COMMAND_STATUS }
	);

	let failure;
	try {
		await runtime.commands.createTodo(
			{ id: 'todo-invalid-output', title: 'invalid' },
			{ commandId: COMMAND_A }
		);
		assert.fail('invalid output should reject');
	} catch (error) {
		failure = error;
	}
	assert.equal(failure.code, 'REPLICA_COMMAND_PROTOCOL_INVALID');
	assert.equal(failure.recovery.commandId, COMMAND_A);
	assert.equal(replica.layers.get(COMMAND_A).state, 'accepted');
	assert.equal((await failure.recovery.status()).state, 'accepted_pending_projection');
	replica.rejectOptimisticLayer(COMMAND_A);
	runtime.dispose();
});
