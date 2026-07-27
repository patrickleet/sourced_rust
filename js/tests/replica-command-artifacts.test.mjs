import assert from 'node:assert/strict';
import test from 'node:test';

import {
	prepareReplicaCommand,
	ReplicaCommandContractError,
	verifyReplicaCommandReceipt
} from '../dist/replica/index.js';

const HASH_A = `sha256:${'a'.repeat(64)}`;
const HASH_B = `sha256:${'b'.repeat(64)}`;
const HASH_C = `sha256:${'c'.repeat(64)}`;
const COMMAND_ID = '018f47de-3d2a-7abc-8abc-0123456789ab';
const OTHER_COMMAND_ID = '018f47de-3d2a-7def-8def-0123456789ab';
const GENERATED_UUID = '018f47de-3d2a-7123-8123-0123456789ab';
const GENERATED_ULID = '01J0Z6YV6E0000000000000000';

const scalarField = (
	name,
	typeName = 'String',
	codec = 'string',
	overrides = {}
) =>
	Object.freeze({
		name,
		typeName,
		nullable: false,
		list: false,
		itemNullable: false,
		codec,
		...overrides
	});

const nestedField = (name, definition, overrides = {}) =>
	Object.freeze({
		name,
		typeName: definition.name,
		nullable: false,
		list: false,
		itemNullable: false,
		nested: definition,
		...overrides
	});

const META_TYPE = Object.freeze({
	name: 'CreateMetaInput',
	fields: Object.freeze([
		scalarField('count', 'Int', 'int32'),
		scalarField('note', 'String', 'string', { nullable: true })
	])
});

const CREATE_TYPE = Object.freeze({
	name: 'CreateTodoInput',
	fields: Object.freeze([
		scalarField('code', 'ID'),
		scalarField('id', 'ID'),
		nestedField('meta', META_TYPE),
		scalarField('title')
	])
});

const OUTPUT = Object.freeze({
	kind: 'object',
	definition: Object.freeze({
		name: 'CommandResult',
		fields: Object.freeze([
			scalarField('value', 'JSON', 'json', { nullable: true })
		])
	})
});
const INPUT = Object.freeze({
	kind: 'object',
	definition: CREATE_TYPE
});
const JSON_IMPORT_INPUT = Object.freeze({
	kind: 'object',
	definition: Object.freeze({
		name: 'ImportTodosInput',
		fields: Object.freeze([scalarField('payload', 'JSON', 'json')])
	})
});
const PROJECTED_OUTPUT = Object.freeze({
	kind: 'object',
	definition: Object.freeze({
		name: 'Todo',
		fields: Object.freeze([
			scalarField('id', 'ID'),
			scalarField('title')
		])
	})
});

const inputValue = (path) => Object.freeze({ kind: 'input', path: Object.freeze(path) });
const constantValue = (value) => Object.freeze({ kind: 'constant', value });
const effectField = (field, value) => Object.freeze({ field, value });
const key = (...fields) => Object.freeze({ fields: Object.freeze(fields) });

const TODO_RELATIONSHIP = Object.freeze({
	sourceModel: 'Todo',
	field: 'related',
	targetModel: 'Todo'
});

function baseArtifact(overrides = {}) {
	return {
		version: 1,
		name: 'todo.create',
		mutationField: 'createTodo',
		document:
			'mutation Client_createTodo($commandId: ID!, $input: CreateTodoInput!) { createTodo(commandId: $commandId, input: $input) }',
		operationHash: HASH_A,
		protocol: {
			version: 1,
			schemaHash: HASH_B,
			protocolHash: HASH_C,
			surface: { kind: 'role', name: 'user' },
			operation: HASH_A,
			trustedPresets: []
		},
		input: INPUT,
		output: OUTPUT,
		inputDefaults: {
			version: 1,
			defaults: [
				{ path: ['code'], generator: 'ulid' },
				{ path: ['id'], generator: 'uuid_v7' }
			]
		},
		consistency: 'fact',
		effects: {
			version: 1,
			operations: [
				{
					kind: 'upsert',
					model: 'Todo',
					key: key(effectField('id', inputValue(['id']))),
					fields: [
						effectField('title', inputValue(['title'])),
						effectField('count', inputValue(['meta', 'count']))
					]
				}
			],
			fallback: 'revalidate'
		},
		confirmations: {
			version: 1,
			kind: 'finite',
			expected: [
				{
					projector: 'todos',
					model: 'Todo',
					key: key(effectField('id', inputValue(['id'])))
				}
			],
			fallback: 'revalidate'
		},
		revalidation: {
			version: 1,
			required: false,
			dependencies: ['todo_rows'],
			models: ['Todo'],
			relationships: []
		},
		...overrides
	};
}

function projectedArtifact(directOverrides = {}) {
	return baseArtifact({
		output: PROJECTED_OUTPUT,
		consistency: 'projected',
		confirmations: undefined,
		directProjection: {
			topology: {
				version: 1,
				name: 'todos',
				digest: HASH_C
			},
			model: 'Todo',
			identityFields: ['id'],
			partition: inputValue(['meta', 'count']),
			changeEpoch: 'todos-v1',
			...directOverrides
		}
	});
}

function receipt(prepared, state, overrides = {}) {
	return {
		commandId: prepared.commandId,
		causationId: 'opaque-causation',
		state,
		consistency: prepared.consistency,
		expects: [],
		observations: [],
		records: [],
		...overrides
	};
}

function expectation(projection, model, token) {
	return { projection, model, scopeToken: token };
}

test('preparation fills omitted UUIDv7/ULID defaults once and closes every input expression', () => {
	let uuidCalls = 0;
	let ulidCalls = 0;
	const callerInput = {
		title: 'Ship it',
		meta: { count: 3 }
	};
	const prepared = prepareReplicaCommand(baseArtifact(), callerInput, {
		commandId: COMMAND_ID,
		generators: {
			uuidV7: () => {
				uuidCalls += 1;
				return GENERATED_UUID;
			},
			ulid: () => {
				ulidCalls += 1;
				return GENERATED_ULID;
			}
		}
	});

	assert.deepEqual(callerInput, { title: 'Ship it', meta: { count: 3 } });
	assert.equal(prepared.input.id, GENERATED_UUID);
	assert.equal(prepared.input.code, GENERATED_ULID);
	assert.equal(prepared.transport.variables.input, prepared.input);
	assert.deepEqual(prepared.transport.variables, {
		commandId: COMMAND_ID,
		input: {
			code: GENERATED_ULID,
			id: GENERATED_UUID,
			meta: { count: 3 },
			title: 'Ship it'
		}
	});
	assert.deepEqual(prepared.optimistic.operations[0], {
		kind: 'upsert',
		model: 'Todo',
		key: { fields: [{ field: 'id', value: GENERATED_UUID }] },
		fields: [
			{ field: 'title', value: 'Ship it' },
			{ field: 'count', value: 3 }
		]
	});
	assert.equal(prepared.confirmations.expected[0].key.fields[0].value, GENERATED_UUID);
	assert.equal(uuidCalls, 1);
	assert.equal(ulidCalls, 1);

	// The prepared value itself is the retry unit; reading/reusing it performs no work.
	assert.equal(prepared.transport.variables.input.id, GENERATED_UUID);
	assert.equal(prepared.optimistic.operations[0].key.fields[0].value, GENERATED_UUID);
	assert.equal(uuidCalls, 1);
	assert.equal(ulidCalls, 1);
});

test('explicit defaulted fields are retained and their generators never run', () => {
	let calls = 0;
	const prepared = prepareReplicaCommand(
		baseArtifact(),
		{
			code: 'caller-code',
			id: 'caller-id',
			meta: { count: 1 },
			title: 'Explicit'
		},
		{
			commandId: COMMAND_ID.toUpperCase(),
			generators: {
				uuidV7: () => {
					calls += 1;
					return GENERATED_UUID;
				},
				ulid: () => {
					calls += 1;
					return GENERATED_ULID;
				}
			}
		}
	);

	assert.equal(prepared.commandId, COMMAND_ID);
	assert.equal(prepared.input.id, 'caller-id');
	assert.equal(prepared.input.code, 'caller-code');
	assert.equal(calls, 0);
});

test('real default generators produce canonical values', () => {
	const prepared = prepareReplicaCommand(
		baseArtifact(),
		{ meta: { count: 1 }, title: 'Generated' },
		{ commandId: COMMAND_ID }
	);

	assert.match(prepared.input.id, /^[0-9a-f]{8}-[0-9a-f]{4}-7/);
	assert.match(prepared.input.code, /^[0-7][0-9A-HJKMNP-TV-Z]{25}$/);
});

test('none inputs and typed JSON fields produce exact canonical transport variables', () => {
	const noInput = prepareReplicaCommand(
		baseArtifact({
			name: 'todo.rebuild',
			mutationField: 'rebuildTodos',
			document:
				'mutation Client_rebuildTodos($commandId: ID!) { rebuildTodos(commandId: $commandId) }',
			input: { kind: 'none' },
			inputDefaults: undefined,
			consistency: 'accepted',
			effects: { version: 1, operations: [], fallback: 'revalidate' },
			confirmations: undefined,
			revalidation: {
				version: 1,
				required: true,
				dependencies: ['todo_rows'],
				models: ['Todo'],
				relationships: []
			}
		}),
		undefined,
		{ commandId: COMMAND_ID }
	);
	assert.deepEqual(noInput.transport.variables, { commandId: COMMAND_ID });
	assert.equal(noInput.input, undefined);

	const json = prepareReplicaCommand(
		baseArtifact({
			name: 'todo.import',
			mutationField: 'importTodos',
			document:
				'mutation Client_importTodos($commandId: ID!, $input: ImportTodosInput!) { importTodos(commandId: $commandId, input: $input) }',
			input: JSON_IMPORT_INPUT,
			inputDefaults: undefined,
			consistency: 'accepted',
			effects: { version: 1, operations: [], fallback: 'revalidate' },
			confirmations: undefined,
			revalidation: {
				version: 1,
				required: true,
				dependencies: ['todo_rows'],
				models: ['Todo'],
				relationships: []
			}
		}),
		{ payload: { z: [2, 1], a: { y: true, x: null } } },
		{ commandId: COMMAND_ID }
	);
	assert.deepEqual(Object.keys(json.input), ['payload']);
	assert.deepEqual(Object.keys(json.input.payload), ['a', 'z']);
	assert.deepEqual(Object.keys(json.input.payload.a), ['x', 'y']);
	assert.equal(json.transport.variables.input, json.input);

	const cyclic = {};
	cyclic.self = cyclic;
	assert.throws(
		() =>
			prepareReplicaCommand(
				baseArtifact({
					name: 'todo.import',
					mutationField: 'importTodos',
					document:
						'mutation Client_importTodos($commandId: ID!, $input: ImportTodosInput!) { importTodos(commandId: $commandId, input: $input) }',
					input: JSON_IMPORT_INPUT,
					inputDefaults: undefined,
					consistency: 'accepted',
					effects: { version: 1, operations: [], fallback: 'revalidate' },
					confirmations: undefined,
					revalidation: {
						version: 1,
						required: true,
						dependencies: ['todo_rows'],
						models: ['Todo'],
						relationships: []
					}
				}),
				{ payload: cyclic },
				{ commandId: COMMAND_ID }
			),
		(error) =>
			error instanceof ReplicaCommandContractError &&
			error.code === 'REPLICA_COMMAND_INPUT_INVALID'
	);
});

test('nested paths resolve exactly and missing or type-invalid values fail closed', () => {
	const nullableMeta = {
		...META_TYPE,
		fields: [
			scalarField('count', 'Int', 'int32'),
			scalarField('note', 'String', 'string', { nullable: true })
		]
	};
	const artifact = baseArtifact({
		input: {
			kind: 'object',
			definition: {
				...CREATE_TYPE,
				fields: [
					scalarField('code', 'ID'),
					scalarField('id', 'ID'),
					nestedField('meta', nullableMeta),
					scalarField('title')
				]
			}
		},
		effects: {
			version: 1,
			operations: [
				{
					kind: 'patch',
					model: 'Todo',
					key: key(effectField('id', inputValue(['id']))),
					fields: [effectField('note', inputValue(['meta', 'note']))]
				}
			],
			fallback: 'revalidate'
		}
	});

	assert.throws(
		() =>
			prepareReplicaCommand(
				artifact,
				{ meta: { count: 1 }, title: 'Absent note' },
				{
					commandId: COMMAND_ID,
					generators: {
						uuidV7: () => GENERATED_UUID,
						ulid: () => GENERATED_ULID
					}
				}
			),
		(error) =>
			error instanceof ReplicaCommandContractError &&
			error.code === 'REPLICA_COMMAND_INPUT_INVALID' &&
			error.path === 'input.meta.note'
	);
	assert.throws(
		() =>
			prepareReplicaCommand(
				baseArtifact(),
				{ meta: { count: 'one' }, title: 'Wrong type' },
				{
					commandId: COMMAND_ID,
					generators: {
						uuidV7: () => GENERATED_UUID,
						ulid: () => GENERATED_ULID
					}
				}
			),
		(error) =>
			error instanceof ReplicaCommandContractError &&
			error.code === 'REPLICA_COMMAND_INPUT_INVALID' &&
			error.path === 'input.meta.count'
	);
	assert.throws(
		() =>
			prepareReplicaCommand(
				baseArtifact(),
				{ extra: true, meta: { count: 1 }, title: 'Unknown field' },
				{
					commandId: COMMAND_ID,
					generators: {
						uuidV7: () => GENERATED_UUID,
						ulid: () => GENERATED_ULID
					}
				}
			),
		(error) =>
			error instanceof ReplicaCommandContractError &&
			error.path === 'input.extra'
	);
});

test('the closed optimistic IR supports every declared effect kind', () => {
	const artifact = baseArtifact({
		effects: {
			version: 1,
			operations: [
				{
					kind: 'upsert',
					model: 'Todo',
					key: key(effectField('id', inputValue(['id']))),
					fields: [effectField('title', inputValue(['title']))]
				},
				{
					kind: 'patch',
					model: 'Todo',
					key: key(effectField('id', inputValue(['id']))),
					fields: [effectField('rank', constantValue(2))]
				},
				{
					kind: 'delete',
					model: 'Todo',
					key: key(effectField('id', inputValue(['id'])))
				},
				{
					kind: 'link',
					relationship: TODO_RELATIONSHIP,
					source: key(effectField('id', inputValue(['id']))),
					target: key(effectField('id', constantValue('target')))
				},
				{
					kind: 'unlink',
					relationship: TODO_RELATIONSHIP,
					source: key(effectField('id', inputValue(['id']))),
					target: key(effectField('id', constantValue('target')))
				},
				{ kind: 'invalidate_model', model: 'Todo' },
				{
					kind: 'invalidate_relationship',
					relationship: TODO_RELATIONSHIP,
					source: key(effectField('id', inputValue(['id'])))
				}
			],
			fallback: 'revalidate'
		},
		revalidation: {
			version: 1,
			required: false,
			dependencies: ['todo_rows', 'todo_links'],
			models: ['Todo'],
			relationships: [TODO_RELATIONSHIP]
		}
	});
	const prepared = prepareReplicaCommand(
		artifact,
		{ meta: { count: 4 }, title: 'All effects' },
		{
			commandId: COMMAND_ID,
			generators: {
				uuidV7: () => GENERATED_UUID,
				ulid: () => GENERATED_ULID
			}
		}
	);

	assert.deepEqual(
		prepared.optimistic.operations.map(({ kind }) => kind),
		[
			'upsert',
			'patch',
			'delete',
			'link',
			'unlink',
			'invalidate_model',
			'invalidate_relationship'
		]
	);
	assert.equal(prepared.optimistic.operations[0].key.fields[0].value, GENERATED_UUID);
	assert.equal(prepared.optimistic.operations[1].fields[0].value, 2);
	assert.equal(prepared.optimistic.operations[3].target.fields[0].value, 'target');
});

test('unknown generators/effects and unresolved trusted presets fail before optimism', () => {
	const options = {
		commandId: COMMAND_ID,
		generators: {
			uuidV7: () => GENERATED_UUID,
			ulid: () => GENERATED_ULID
		}
	};
	const input = { meta: { count: 1 }, title: 'Invalid IR' };

	assert.throws(
		() =>
			prepareReplicaCommand(
				baseArtifact({
					protocol: {
						version: 1,
						schemaHash: HASH_B,
						protocolHash: HASH_C,
						operation: HASH_A,
						trustedPresets: []
					}
				}),
				input,
				options
			),
		(error) =>
			error instanceof ReplicaCommandContractError &&
			error.path === 'artifact.protocol.surface'
	);
	assert.throws(
		() =>
			prepareReplicaCommand(
				baseArtifact({
					protocol: {
						version: 1,
						schemaHash: HASH_B,
						protocolHash: HASH_C,
						surface: { kind: 'role', name: 'user' },
						operation: HASH_B,
						trustedPresets: []
					}
				}),
				input,
				options
			),
		(error) =>
			error instanceof ReplicaCommandContractError &&
			error.path === 'artifact.protocol.operation'
	);
	assert.throws(
		() =>
			prepareReplicaCommand(
				baseArtifact({
					inputDefaults: {
						version: 1,
						defaults: [{ path: ['id'], generator: 'uuid_v4' }]
					}
				}),
				input,
				options
			),
		(error) =>
			error instanceof ReplicaCommandContractError &&
			error.code === 'REPLICA_COMMAND_ARTIFACT_INVALID'
	);
	assert.throws(
		() =>
			prepareReplicaCommand(
				baseArtifact({
					effects: {
						version: 1,
						operations: [{ kind: 'explode' }],
						fallback: 'revalidate'
					}
				}),
				input,
				options
			),
		(error) =>
			error instanceof ReplicaCommandContractError &&
			error.path === 'artifact.effects.operations[0].kind'
	);
	assert.throws(
		() =>
			prepareReplicaCommand(
				baseArtifact({
					effects: {
						version: 1,
						operations: [
							{
								kind: 'patch',
								model: 'Todo',
								key: key(effectField('id', inputValue(['id']))),
								fields: [
									effectField('owner', {
										kind: 'trusted_preset',
										name: 'current_tenant'
									})
								]
							}
						],
						fallback: 'revalidate'
					}
				}),
				input,
				options
			),
		(error) =>
			error instanceof ReplicaCommandContractError &&
			error.code === 'REPLICA_COMMAND_ARTIFACT_INVALID'
	);
	assert.throws(
		() =>
			prepareReplicaCommand(baseArtifact(), input, {
				commandId: COMMAND_ID,
				generators: {
					uuidV7: () => 'not-a-uuid',
					ulid: () => GENERATED_ULID
				}
			}),
		(error) =>
			error instanceof ReplicaCommandContractError &&
			error.code === 'REPLICA_COMMAND_INPUT_INVALID'
	);
});

test('finite receipts compare projector/model as an exact multiset including multiplicity', () => {
	const finiteArtifact = baseArtifact({
		confirmations: {
			version: 1,
			kind: 'finite',
			expected: [
				{
					projector: 'todos',
					model: 'Todo',
					key: key(effectField('id', inputValue(['id'])))
				},
				{
					projector: 'todos',
					model: 'Todo',
					key: key(effectField('id', inputValue(['code'])))
				},
				{
					projector: 'search',
					model: 'TodoSearch',
					key: key(effectField('id', inputValue(['id'])))
				}
			],
			fallback: 'revalidate'
		}
	});
	const prepared = prepareReplicaCommand(
		finiteArtifact,
		{ meta: { count: 1 }, title: 'Receipt' },
		{
			commandId: COMMAND_ID,
			generators: {
				uuidV7: () => GENERATED_UUID,
				ulid: () => GENERATED_ULID
			}
		}
	);

	assert.deepEqual(
		verifyReplicaCommandReceipt(prepared, receipt(prepared, 'in_progress')),
		{ kind: 'deferred', revalidate: false }
	);
	assert.deepEqual(
		verifyReplicaCommandReceipt(
			prepared,
			receipt(prepared, 'accepted_pending_projection', {
				expects: [
					expectation('search', 'TodoSearch', 'token-3'),
					expectation('todos', 'Todo', 'token-1'),
					expectation('todos', 'Todo', 'token-2')
				]
			})
		),
		{ kind: 'matched', revalidate: false }
	);
	assert.throws(
		() =>
			verifyReplicaCommandReceipt(
				prepared,
				receipt(prepared, 'accepted_pending_projection', {
					expects: [
						expectation('search', 'TodoSearch', 'token-3'),
						expectation('todos', 'Todo', 'token-1')
					]
				})
			),
		(error) =>
			error instanceof ReplicaCommandContractError &&
			error.code === 'REPLICA_COMMAND_RECEIPT_MISMATCH' &&
			error.path === 'receipt.expects'
	);
	assert.throws(
		() =>
			verifyReplicaCommandReceipt(
				prepared,
				receipt(prepared, 'accepted_pending_projection')
			),
		(error) =>
			error instanceof ReplicaCommandContractError &&
			error.path === 'receipt.expects'
	);
	assert.throws(
		() =>
			verifyReplicaCommandReceipt(prepared, {
				...receipt(prepared, 'accepted_pending_projection'),
				commandId: OTHER_COMMAND_ID
			}),
		(error) =>
			error instanceof ReplicaCommandContractError &&
			error.path === 'receipt.commandId'
	);
	assert.throws(
		() =>
			verifyReplicaCommandReceipt(prepared, {
				...receipt(prepared, 'accepted_pending_projection'),
				consistency: 'accepted'
			}),
		(error) =>
			error instanceof ReplicaCommandContractError &&
			error.path === 'receipt.consistency'
	);
});

test('unavailable confirmation contracts always force conservative revalidation', () => {
	const artifact = baseArtifact({
		consistency: 'accepted',
		confirmations: {
			version: 1,
			kind: 'unavailable',
			expected: [],
			fallback: 'revalidate'
		},
		revalidation: {
			version: 1,
			required: true,
			dependencies: ['todo_rows'],
			models: ['Todo'],
			relationships: []
		}
	});
	const prepared = prepareReplicaCommand(
		artifact,
		{ meta: { count: 1 }, title: 'Unavailable' },
		{
			commandId: COMMAND_ID,
			generators: {
				uuidV7: () => GENERATED_UUID,
				ulid: () => GENERATED_ULID
			}
		}
	);

	assert.deepEqual(
		verifyReplicaCommandReceipt(
			prepared,
			receipt(prepared, 'accepted', {
				expects: [expectation('hidden', 'Hidden', 'opaque')]
			})
		),
		{
			kind: 'revalidate',
			revalidate: true,
			reason: 'confirmation_unavailable'
		}
	);
});

test('accepted commands without finite confirmations require canonical revalidation', () => {
	const invalid = baseArtifact({
		consistency: 'accepted',
		confirmations: undefined
	});
	const input = { meta: { count: 1 }, title: 'No finite fence' };
	const options = {
		commandId: COMMAND_ID,
		generators: {
			uuidV7: () => GENERATED_UUID,
			ulid: () => GENERATED_ULID
		}
	};

	assert.throws(
		() => prepareReplicaCommand(invalid, input, options),
		(error) =>
			error instanceof ReplicaCommandContractError &&
			error.path === 'artifact.revalidation.required'
	);

	const valid = baseArtifact({
		consistency: 'accepted',
		confirmations: undefined,
		revalidation: {
			...invalid.revalidation,
			required: true
		}
	});
	assert.equal(
		prepareReplicaCommand(valid, input, options).revalidation.required,
		true
	);
});

test('projected commands close the direct projection partition from finalized input', () => {
	const artifact = projectedArtifact();
	const prepared = prepareReplicaCommand(
		artifact,
		{ meta: { count: 7 }, title: 'Projected' },
		{
			commandId: COMMAND_ID,
			generators: {
				uuidV7: () => GENERATED_UUID,
				ulid: () => GENERATED_ULID
			}
		}
	);

	assert.equal(prepared.directProjection.partition, 7);
	assert.equal(prepared.directProjection.topology.name, 'todos');
	assert.deepEqual(prepared.directProjection.identityFields, ['id']);
	assert.notEqual(
		prepared.directProjection.identityFields,
		artifact.directProjection.identityFields
	);
	assert.equal(Object.isFrozen(prepared.directProjection), true);
	assert.equal(Object.isFrozen(prepared.directProjection.topology), true);
	assert.equal(Object.isFrozen(prepared.directProjection.identityFields), true);
	assert.deepEqual(
		verifyReplicaCommandReceipt(prepared, receipt(prepared, 'projected')),
		{ kind: 'matched', revalidate: false }
	);
});

test('projected command identity facts reject missing, unknown, and duplicate fields', () => {
	const input = { meta: { count: 7 }, title: 'Projected' };
	const options = {
		commandId: COMMAND_ID,
		generators: {
			uuidV7: () => GENERATED_UUID,
			ulid: () => GENERATED_ULID
		}
	};
	const cases = [
		{
			identityFields: undefined,
			path: 'artifact.directProjection.identityFields'
		},
		{ identityFields: [], path: 'artifact.directProjection.identityFields' },
		{
			identityFields: ['missing'],
			path: 'artifact.directProjection.identityFields[0]'
		},
		{
			identityFields: ['id', 'id'],
			path: 'artifact.directProjection.identityFields[1]'
		}
	];

	for (const { identityFields, path } of cases) {
		assert.throws(
			() =>
				prepareReplicaCommand(
					projectedArtifact({ identityFields }),
					input,
					options
				),
			(error) =>
				error instanceof ReplicaCommandContractError &&
				error.code === 'REPLICA_COMMAND_ARTIFACT_INVALID' &&
				error.path === path
		);
	}

	assert.throws(
		() =>
			prepareReplicaCommand(
				{
					...projectedArtifact(),
					output: OUTPUT
				},
				input,
				options
			),
		(error) =>
			error instanceof ReplicaCommandContractError &&
			error.code === 'REPLICA_COMMAND_ARTIFACT_INVALID' &&
			error.path === 'artifact.directProjection.identityFields[0]'
	);
});

test('prepared command input, variables, effects, confirmations, and plans are deeply frozen', () => {
	const prepared = prepareReplicaCommand(
		baseArtifact(),
		{ meta: { count: 2, note: 'nested' }, title: 'Frozen' },
		{
			commandId: COMMAND_ID,
			generators: {
				uuidV7: () => GENERATED_UUID,
				ulid: () => GENERATED_ULID
			}
		}
	);

	for (const value of [
		prepared,
		prepared.input,
		prepared.input.meta,
		prepared.transport,
		prepared.transport.protocol,
		prepared.transport.variables,
		prepared.optimistic,
		prepared.optimistic.operations,
		prepared.optimistic.operations[0],
		prepared.optimistic.operations[0].key,
		prepared.optimistic.operations[0].key.fields,
		prepared.confirmations,
		prepared.confirmations.expected,
		prepared.confirmations.expected[0],
		prepared.revalidation,
		prepared.revalidation.dependencies,
		prepared.revalidation.models,
		prepared.revalidation.relationships
	]) {
		assert.equal(Object.isFrozen(value), true);
	}
	assert.throws(() => {
		prepared.input.meta.count = 9;
	}, TypeError);
});
