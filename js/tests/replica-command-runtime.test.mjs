import assert from 'node:assert/strict';
import test from 'node:test';

import { createCacheEngine } from '../dist/internal/cache-engine.js';
import {
	createReplicaCommandRuntime,
	replicaCommandAuthority,
	replicaCommandDirectProjection,
	replicaCommandProjectionDelta,
	ReplicaCommandRuntimeError
} from '../dist/replica/command-runtime.js';
import {
	prepareReplicaCommand,
	replicaRecordKey,
	ReplicaCommandContractError
} from '../dist/replica/index.js';

const HASH_A = `sha256:${'a'.repeat(64)}`;
const HASH_B = `sha256:${'b'.repeat(64)}`;
const HASH_C = `sha256:${'c'.repeat(64)}`;
const HASH_D = `sha256:${'d'.repeat(64)}`;
const PROGRAM = `pp1:sha256:${'1'.repeat(64)}`;
const BINDING = `pb1:sha256:${'2'.repeat(64)}`;
const COMMAND_A = '018f47de-3d2a-7abc-8abc-0123456789ab';
const COMMAND_B = '018f47de-3d2a-7def-8def-0123456789ab';
const COMMAND_C = '018f47de-3d2a-7123-8123-0123456789ab';
const SURFACE = Object.freeze({ kind: 'role', name: 'user' });
const CACHE_SCOPE = token('cache-scope', 1);
const Todo = Object.freeze({
	id: 'Todos',
	identityFields: Object.freeze(['id'])
});

const scalar = (name, typeName = 'String', codec = 'string') =>
	Object.freeze({
		name,
		typeName,
		codec,
		nullable: false,
		list: false,
		itemNullable: false
	});

const TodoInput = Object.freeze({
	name: 'TodoInput',
	fields: Object.freeze([scalar('id', 'ID'), scalar('title')])
});
const ResultOutput = Object.freeze({
	name: 'CommandResult',
	fields: Object.freeze([scalar('ok', 'Boolean', 'boolean')])
});

const input = (path) =>
	Object.freeze({ kind: 'input', path: Object.freeze(path) });
const unit = Object.freeze({ kind: 'unit' });

function scope(value) {
	return Object.freeze({
		partition: unit,
		model: Todo.id,
		key: Object.freeze([
			Object.freeze({
				ordinal: 0,
				field: 'id',
				value
			})
		])
	});
}

function projection(operation = 'upsert') {
	const event = Object.freeze({ id: 'event-1', name: 'todo.changed', version: 1 });
	const previewScope = scope(input(['id']));
	let mutation;
	if (operation === 'upsert') {
		mutation = Object.freeze({
			op: 'upsert',
			scope: previewScope,
			fields: Object.freeze([
				Object.freeze({ field: 'title', value: input(['title']) })
			]),
			replace: Object.freeze(['title'])
		});
	} else if (operation === 'patch') {
		mutation = Object.freeze({
			op: 'patch',
			scope: previewScope,
			set: Object.freeze([
				Object.freeze({ field: 'title', value: input(['title']) })
			]),
			unset: Object.freeze([]),
			if_present: true
		});
	} else {
		mutation = Object.freeze({ op: 'delete', scope: previewScope });
	}
	return Object.freeze({
		version: 2,
		deltaWireVersion: 1,
		projectionProgramVersion: 2,
		operationSemanticsVersion: 1,
		projections: Object.freeze([
			Object.freeze({
				programId: PROGRAM,
				bindingId: BINDING,
				epoch: 'todos-v1',
				programIrVersion: 1,
				operationSemanticsVersion: 1
			})
		]),
		eventSet: Object.freeze([event]),
		preview: Object.freeze({
			version: 1,
			occurrences: Object.freeze([
				Object.freeze({ ordinal: 0, event })
			]),
			operations: Object.freeze([
				Object.freeze({
					occurrence_ordinal: 0,
					projection_refs: Object.freeze([0]),
					mutation
				})
			]),
			recoveries: Object.freeze([])
		}),
		fallback: 'revalidate'
	});
}

function artifact(options = {}) {
	const operation = options.operation ?? 'upsert';
	return Object.freeze({
		version: 2,
		name: options.name ?? `todo.${operation}`,
		mutationField: options.mutationField ?? 'changeTodo',
		document:
			'mutation Client_changeTodo($commandId: ID!, $input: TodoInput!) { changeTodo(commandId: $commandId, input: $input) { ok } }',
		operationHash: HASH_A,
		protocol: Object.freeze({
			version: 1,
			schemaHash: HASH_B,
			protocolHash: HASH_C,
			surface: SURFACE,
			operation: HASH_A,
			trustedPresets: Object.freeze([])
		}),
		input: Object.freeze({ kind: 'object', definition: TodoInput }),
		output: Object.freeze({ kind: 'object', definition: ResultOutput }),
		consistency: options.consistency ?? 'causal',
		...(options.modeled === false ? {} : { projection: projection(operation) }),
		...(options.directProjection === undefined
			? {}
			: { directProjection: options.directProjection }),
		revalidation: Object.freeze({
			version: 1,
			required: options.revalidate ?? false,
			dependencies: Object.freeze(['todos']),
			models: Object.freeze([Todo.id]),
			relationships: Object.freeze([])
		})
	});
}

function deltaMutation(request, options = {}) {
	const operation = options.operation ?? 'upsert';
	const actualScope = scope({
		type: 'string',
		value: request.variables.input.id
	});
	if (operation === 'upsert') {
		return {
			op: 'upsert',
			scope: actualScope,
			fields: [
				{
					field: 'title',
					value: {
						type: 'string',
						value: options.actualTitle ?? request.variables.input.title
					}
				}
			],
			replace: ['title']
		};
	}
	if (operation === 'patch') {
		return {
			op: 'patch',
			scope: actualScope,
			set: [
				{
					field: 'title',
					value: {
						type: 'string',
						value: options.actualTitle ?? request.variables.input.title
					}
				}
			],
			unset: [],
			if_present: true
		};
	}
	return { op: 'delete', scope: actualScope };
}

function commandMetadata(request, options = {}) {
	const state = options.state ?? 'succeeded_pending_projection';
	const causationId = options.causationId ?? `cause:${request.commandId}`;
	if (state === 'in_progress' && options.projection === false) {
		return {
			commandId: request.commandId,
			causationId,
			state,
			consistency: options.consistency ?? 'causal',
			expects: [],
			observations: [],
			records: []
		};
	}
	const obligations = Array.from(
		{ length: options.obligations ?? 1 },
		(_, index) => ({
			projectionRef: 0,
			model: Todo.id,
			scopeToken: token('projection-obligation', index + 3)
		})
	);
	const projectionMetadata = {
		wireVersion: 1,
		issuedAtUnixMs: Date.now() - 1_000,
		expiresAtUnixMs: Date.now() + 60_000,
		delta: {
			wire_version: 1,
			identity: {
				manifest_version: 2,
				client_protocol_version: 1,
				surface: options.surface ?? SURFACE,
				schema_fingerprint: options.schemaHash ?? HASH_B,
				protocol_fingerprint: options.protocolHash ?? HASH_C,
				authorization_generation: options.authorizationGeneration ?? 'auth-1',
				cache_scope_token: options.cacheScope ?? CACHE_SCOPE,
				command_causation_id: causationId
			},
			projections: [
				{
					program_id: PROGRAM,
					binding_id: options.bindingId ?? BINDING,
					epoch: 'todos-v1',
					program_ir_version: 1,
					operation_semantics_version: 1
				}
			],
			occurrences: [
				{
					causation_id: causationId,
					ordinal: 0,
					occurrence_id: `occurrence:${request.commandId}`
				}
			],
			operations: options.emptyDelta
				? []
				: [
						{
							occurrence_ordinal: 0,
							projection_refs: [0],
							mutation: deltaMutation(request, options)
						}
					]
		},
		obligations,
		revalidate: options.revalidate ?? false
	};
	return {
		commandId: request.commandId,
		causationId,
		state,
		consistency: options.consistency ?? 'causal',
		expects: obligations.map((obligation) => ({
			projection: PROGRAM,
			model: obligation.model,
			scopeToken: obligation.scopeToken
		})),
		observations: options.observations ?? [],
		records: options.records ?? [],
		projection: projectionMetadata
	};
}

function envelope(request, options = {}) {
	const metadata =
		options.command ??
		commandMetadata(request, options);
	return {
		status: 200,
		data:
			options.data ??
			{ [request.mutationField]: { ok: true } },
		extensions: {
			distributed: {
				protocolVersion: 1,
				schemaHash: HASH_B,
				cacheScope: CACHE_SCOPE,
				operation: request.operationHash,
				trustedPresets: [],
				command: metadata
			}
		}
	};
}

function statusEnvelope(request, metadata) {
	return {
		status: 200,
		data: { commandStatus: { state: metadata.state } },
		extensions: {
			distributed: {
				protocolVersion: 1,
				schemaHash: HASH_B,
				cacheScope: CACHE_SCOPE,
				operation: HASH_D,
				trustedPresets: [],
				command: metadata
			}
		}
	};
}

const STATUS = Object.freeze({
	name: 'Distributed_CommandStatus',
	document:
		'query Distributed_CommandStatus($commandId: ID!) { commandStatus(commandId: $commandId) { state } }',
	operationHash: HASH_D,
	protocol: Object.freeze({
		version: 1,
		schemaHash: HASH_B,
		protocolHash: HASH_C,
		surface: SURFACE,
		operation: HASH_D,
		trustedPresets: Object.freeze([])
	})
});

class TestReplica {
	engine = createCacheEngine();
	authorizationGeneration = 1;
	scope = Object.freeze({
		protocolVersion: 1,
		schemaHash: HASH_B,
		cacheScope: CACHE_SCOPE
	});
	revalidations = [];
	replacements = [];
	direct = [];
	#controller = new AbortController();

	[replicaCommandAuthority]() {
		return Object.freeze({
			read: () => ({
				generation: this.authorizationGeneration,
				scope: this.scope,
				trustedPresets: Object.freeze([]),
				signal: this.#controller.signal
			}),
			dispose() {}
		});
	}

	createOptimisticLayer(id, update) {
		this.engine.createOptimisticLayer(id, (writer) =>
			update(replicaWriter(writer))
		);
	}

	[replicaCommandProjectionDelta](id, update, semanticChanges) {
		this.replacements.push({ id, semanticChanges });
		return this.engine.replaceOptimisticLayer(id, (_reader, writer) =>
			update(replicaWriter(writer))
		);
	}

	[replicaCommandDirectProjection](id, projectionValue) {
		this.direct.push(projectionValue);
		this.engine.confirmOptimisticLayer(id, (writer) =>
			writer.writeRecord({
				key: replicaRecordKey(projectionValue.model, projectionValue.identity),
				revision: projectionValue.evidence.revision,
				incarnation: projectionValue.evidence.incarnation,
				fields: projectionValue.fields
			})
		);
	}

	markOptimisticLayerAccepted(id) {
		return this.engine.markOptimisticLayerAccepted(id);
	}

	confirmOptimisticLayer(id, update) {
		return this.engine.confirmOptimisticLayer(id, update);
	}

	rejectOptimisticLayer(id) {
		return this.engine.rejectOptimisticLayer(id);
	}

	revalidate(plan) {
		this.revalidations.push(plan);
		return Promise.resolve();
	}

	record(id) {
		return this.engine.read((reader) =>
			reader.record(replicaRecordKey(Todo, [id]))
		);
	}

	layer(id) {
		return this.engine.optimisticLayerState(id);
	}

	invalidate() {
		this.authorizationGeneration += 1;
		this.#controller.abort(new Error('scope changed'));
	}
}

function replicaWriter(writer) {
	return Object.freeze({
		writeRecord(model, identity, patch) {
			writer.writeRecord({
				key: replicaRecordKey(model, identity),
				fields: patch.fields,
				unset: patch.unset,
				ifPresent: patch.ifPresent
			});
		},
		tombstoneRecord(model, identity) {
			writer.tombstoneRecord(replicaRecordKey(model, identity));
		},
		writeIndex() {},
		deleteIndex() {}
	});
}

function deferred() {
	let resolve;
	let reject;
	const promise = new Promise((yes, no) => {
		resolve = yes;
		reject = no;
	});
	return { promise, resolve, reject };
}

function token(purpose, byte) {
	return `v1.${purpose}.${Buffer.alloc(32, byte).toString('base64url')}`;
}

function tick() {
	return new Promise((resolve) => setTimeout(resolve, 0));
}

test('artifact v1 is rejected at the public boundary', () => {
	assert.throws(
		() =>
			prepareReplicaCommand(
				{
					...artifact(),
					version: 1,
					effects: { version: 1, operations: [], fallback: 'revalidate' }
				},
				{ id: 'todo-1', title: 'preview' },
				{ commandId: COMMAND_A }
			),
		(error) =>
			error instanceof ReplicaCommandContractError &&
			error.path === 'artifact.version'
	);
});

test('actual delta atomically replaces preview and later layers rebase in order', async () => {
	const replica = new TestReplica();
	const first = deferred();
	const second = deferred();
	let dispatches = 0;
	const runtime = createReplicaCommandRuntime(
		replica,
		{
			dispatch(request) {
				dispatches += 1;
				return dispatches === 1 ? first.promise : second.promise;
			}
		},
		{ change: artifact() }
	);
	const a = runtime.commands.change(
		{ id: 'todo-1', title: 'preview-a' },
		{ commandId: COMMAND_A }
	);
	const b = runtime.commands.change(
		{ id: 'todo-1', title: 'preview-b' },
		{ commandId: COMMAND_B }
	);
	assert.equal(replica.record('todo-1').fields.title, 'preview-b');

	first.resolve(
		envelope(
			{
				commandId: COMMAND_A,
				mutationField: 'changeTodo',
				operationHash: HASH_A,
				variables: { input: { id: 'todo-1', title: 'preview-a' } }
			},
			{ actualTitle: 'actual-a' }
		)
	);
	const receipt = await a;
	assert.equal(receipt.metadata.expects.length, 1);
	assert.equal(replica.layer(COMMAND_A), 'accepted');
	assert.equal(replica.record('todo-1').fields.title, 'preview-b');

	second.resolve(
		envelope(
			{
				commandId: COMMAND_B,
				mutationField: 'changeTodo',
				operationHash: HASH_A,
				variables: { input: { id: 'todo-1', title: 'preview-b' } }
			},
			{ state: 'rejected', projection: false, obligations: 0 }
		)
	);
	await assert.rejects(b, { code: 'REPLICA_COMMAND_REJECTED' });
	assert.equal(replica.record('todo-1').fields.title, 'actual-a');
	assert.equal(replica.replacements.length, 1);
	runtime.dispose();
});

test('rejecting an earlier create suppresses a dependent conditional patch', async () => {
	const replica = new TestReplica();
	const createResult = deferred();
	const patchResult = deferred();
	const independentResult = deferred();
	const runtime = createReplicaCommandRuntime(
		replica,
		{
			dispatch(request) {
				if (request.commandName === 'todo.upsert') {
					return request.commandId === COMMAND_C
						? independentResult.promise
						: createResult.promise;
				}
				return patchResult.promise;
			}
		},
		{
			create: artifact(),
			patch: artifact({
				name: 'todo.patch',
				mutationField: 'patchTodo',
				operation: 'patch'
			})
		}
	);
	const create = runtime.commands.create(
		{ id: 'dependent', title: 'create' },
		{ commandId: COMMAND_A }
	);
	const patch = runtime.commands.patch(
		{ id: 'dependent', title: 'patch' },
		{ commandId: COMMAND_B }
	);
	const independent = runtime.commands.create(
		{ id: 'independent', title: 'safe' },
		{ commandId: COMMAND_C }
	);
	assert.equal(replica.record('dependent').fields.title, 'patch');
	assert.equal(replica.record('independent').fields.title, 'safe');

	createResult.resolve(
		envelope(
			{
				commandId: COMMAND_A,
				mutationField: 'changeTodo',
				operationHash: HASH_A,
				variables: { input: { id: 'dependent', title: 'create' } }
			},
			{ state: 'rejected', projection: false }
		)
	);
	await assert.rejects(create, { code: 'REPLICA_COMMAND_REJECTED' });
	assert.equal(replica.record('dependent'), undefined);
	assert.equal(replica.record('independent').fields.title, 'safe');

	runtime.dispose();
	await assert.rejects(patch, { code: 'REPLICA_COMMAND_DISPOSED' });
	await assert.rejects(independent, { code: 'REPLICA_COMMAND_DISPOSED' });
});

test('status replay is idempotent only for byte-identical actual metadata', async () => {
	const replica = new TestReplica();
	let statusCalls = 0;
	let actual;
	const runtime = createReplicaCommandRuntime(
		replica,
		{
			dispatch: (request) =>
				Promise.resolve(
					envelope(request, {
						command: commandMetadata(request, {
							state: 'in_progress',
							projection: false
						})
					})
				),
			status(request) {
				statusCalls += 1;
				const commandRequest = {
					commandId: request.commandId,
					mutationField: 'changeTodo',
					operationHash: HASH_A,
					variables: { input: { id: 'todo-1', title: 'preview' } }
				};
				actual ??= commandMetadata(commandRequest, {
					actualTitle: 'actual'
				});
				const metadata =
					statusCalls < 3
						? actual
						: commandMetadata(commandRequest, {
								actualTitle: 'changed'
							});
				return Promise.resolve(statusEnvelope(request, metadata));
			}
		},
		{ change: artifact() },
		{ status: STATUS }
	);
	let recovery;
	await assert.rejects(
		runtime.commands.change(
			{ id: 'todo-1', title: 'preview' },
			{ commandId: COMMAND_A }
		),
		(error) => {
			recovery = error.recovery;
			return (
				error instanceof ReplicaCommandRuntimeError &&
				error.code === 'REPLICA_COMMAND_OUTCOME_PENDING'
			);
		}
	);
	assert.equal((await recovery.status()).state, 'succeeded_pending_projection');
	assert.equal(replica.record('todo-1').fields.title, 'actual');
	assert.equal((await recovery.status()).state, 'succeeded_pending_projection');
	await assert.rejects(recovery.status(), {
		code: 'REPLICA_COMMAND_PROTOCOL_INVALID'
	});
	runtime.dispose();
});

test('surface, digest, scope, causation, and expiry mismatches fail before mutation', async () => {
	for (const override of [
		{ surface: { kind: 'role', name: 'admin' } },
		{ bindingId: `pb1:sha256:${'9'.repeat(64)}` },
		{ cacheScope: token('cache-scope', 9) },
		{ protocolHash: HASH_D }
	]) {
		const replica = new TestReplica();
		const runtime = createReplicaCommandRuntime(
			replica,
			{
				dispatch: (request) =>
					Promise.resolve(envelope(request, override))
			},
			{ change: artifact() }
		);
		await assert.rejects(
			runtime.commands.change(
				{ id: 'todo-1', title: 'preview' },
				{ commandId: COMMAND_A }
			),
			{ code: 'REPLICA_COMMAND_PROTOCOL_INVALID' }
		);
		assert.equal(replica.record('todo-1'), undefined);
		assert.equal(replica.replacements.length, 0);
		runtime.dispose();
	}
});

test('zero, one, and many obligations are server-derived and never predicted keys', async () => {
	for (const count of [0, 1, 3]) {
		const replica = new TestReplica();
		const runtime = createReplicaCommandRuntime(
			replica,
			{
				dispatch: (request) =>
					Promise.resolve(envelope(request, { obligations: count }))
			},
			{ change: artifact() }
		);
		const receipt = await runtime.commands.change(
			{ id: `todo-${count}`, title: 'preview' },
			{ commandId: [COMMAND_A, COMMAND_B, COMMAND_C][count === 3 ? 2 : count] }
		);
		assert.equal(receipt.metadata.expects.length, count);
		assert.equal(receipt.projected === undefined, count === 0);
		if (count === 0) {
			await tick();
			assert.equal(replica.revalidations.length, 1);
			assert.equal(replica.layer(receipt.commandId), undefined);
		}
		runtime.dispose();
	}
});

test('delete remains a provisional tombstone while its causal obligation is pending', async () => {
	const replica = new TestReplica();
	replica.engine.batch((writer) =>
		writer.writeRecord({
			key: replicaRecordKey(Todo, ['todo-1']),
			revision: 1,
			fields: { __typename: Todo.id, id: 'todo-1', title: 'base' }
		})
	);
	const runtime = createReplicaCommandRuntime(
		replica,
		{
			dispatch: (request) =>
				Promise.resolve(envelope(request, { operation: 'delete' }))
		},
		{
			remove: artifact({
				name: 'todo.delete',
				operation: 'delete'
			})
		}
	);
	const receipt = await runtime.commands.remove(
		{ id: 'todo-1', title: 'unused' },
		{ commandId: COMMAND_A }
	);
	assert.equal(replica.record('todo-1'), undefined);
	assert.equal(replica.layer(COMMAND_A), 'accepted');
	assert.equal(receipt.projected instanceof Promise, true);
	runtime.dispose();
});

test('direct Projected results retain the canonical record-clock path', async () => {
	const replica = new TestReplica();
	const directArtifact = Object.freeze({
		...artifact({
			name: 'todo.project',
			consistency: 'projected',
			modeled: false,
			directProjection: Object.freeze({
				topology: Object.freeze({
					version: 1,
					name: 'todos',
					digest: HASH_D
				}),
				model: Todo.id,
				identityFields: Todo.identityFields,
				changeEpoch: 'todos-v1'
			})
		}),
		output: Object.freeze({
			kind: 'object',
			definition: Object.freeze({
				name: Todo.id,
				fields: Object.freeze([scalar('id', 'ID'), scalar('title')])
			})
		})
	});
	const runtime = createReplicaCommandRuntime(
		replica,
		{
			dispatch(request) {
				const metadata = {
					commandId: request.commandId,
					causationId: `cause:${request.commandId}`,
					state: 'projected',
					consistency: 'projected',
					expects: [],
					observations: [],
					records: [
						{
							model: Todo.id,
							scopeToken: token('record-scope', 7),
							incarnation: '1',
							revision: '2',
							tombstone: false
						}
					]
				};
				return Promise.resolve(
					envelope(request, {
						command: metadata,
						data: {
							[request.mutationField]: {
								id: request.variables.input.id,
								title: 'canonical'
							}
						}
					})
				);
			}
		},
		{ project: directArtifact }
	);
	const receipt = await runtime.commands.project(
		{ id: 'todo-1', title: 'preview' },
		{ commandId: COMMAND_A }
	);
	assert.equal(receipt.state, 'projected');
	assert.equal(replica.direct.length, 1);
	assert.equal(replica.record('todo-1').fields.title, 'canonical');
	assert.equal(replica.layer(COMMAND_A), undefined);
	runtime.dispose();
});

test('authorization rollover aborts the captured generation and rolls back preview', async () => {
	const replica = new TestReplica();
	const pending = deferred();
	const runtime = createReplicaCommandRuntime(
		replica,
		{ dispatch: () => pending.promise },
		{ change: artifact() }
	);
	const command = runtime.commands.change(
		{ id: 'todo-1', title: 'preview' },
		{ commandId: COMMAND_A }
	);
	assert.equal(replica.record('todo-1').fields.title, 'preview');
	replica.invalidate();
	await assert.rejects(command, { code: 'REPLICA_COMMAND_SCOPE_INVALIDATED' });
	assert.equal(replica.record('todo-1'), undefined);
	runtime.dispose();
});

test('pre-abort and disposal cancel work without leaking optimistic layers', async () => {
	const replica = new TestReplica();
	const pending = deferred();
	const runtime = createReplicaCommandRuntime(
		replica,
		{ dispatch: () => pending.promise },
		{ change: artifact() }
	);
	const aborted = new AbortController();
	aborted.abort(new Error('caller cancelled'));
	await assert.rejects(
		runtime.commands.change(
			{ id: 'never', title: 'never' },
			{ commandId: COMMAND_A, signal: aborted.signal }
		),
		{ code: 'REPLICA_COMMAND_ABORTED' }
	);
	assert.equal(replica.layer(COMMAND_A), undefined);

	const inFlight = runtime.commands.change(
		{ id: 'todo-1', title: 'preview' },
		{ commandId: COMMAND_B }
	);
	assert.equal(replica.layer(COMMAND_B), 'optimistic');
	runtime.dispose();
	await assert.rejects(inFlight, { code: 'REPLICA_COMMAND_DISPOSED' });
	assert.equal(replica.layer(COMMAND_B), undefined);
});
