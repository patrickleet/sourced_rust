import assert from 'node:assert/strict';
import test from 'node:test';

import {
	DISTRIBUTED_PROTOCOL_VERSION,
	DistributedProtocolError,
	parseDistributedProtocolEnvelope
} from '../dist/index.js';
import {
	createDistributedReplica,
	replicaRecordKey
} from '../dist/replica/index.js';

const Todo = Object.freeze({
	id: 'TodoView',
	identityFields: Object.freeze(['id'])
});

const NoVariables = Object.freeze({
	version: 2,
	limits: Object.freeze({
		maxDepth: 8,
		maxBoolWidth: 256,
		maxInList: 1000
	}),
	variables: Object.freeze({}),
	inputs: Object.freeze({})
});

const Todos = Object.freeze({
	id: 'query:todos',
	document: 'query Todos { todos { id title } }',
	protocol: Object.freeze({
		version: 2,
		schemaHash: 'schema-a',
		surface: Object.freeze({ kind: 'role', name: 'user' }),
		operation: 'query:todos',
		trustedPresets: Object.freeze([])
	}),
	variableCodec: NoVariables,
	live: Object.freeze({
		id: 'live:todos',
		document: 'subscription TodosLive { todos { id title } }'
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

const TodosOtherOperation = Object.freeze({
	...Todos,
	id: 'query:todos-other',
	protocol: Object.freeze({
		version: 2,
		schemaHash: 'schema-a',
		surface: Object.freeze({ kind: 'role', name: 'user' }),
		operation: 'query:todos-other',
		trustedPresets: Object.freeze([])
	}),
	live: undefined
});

function wireFrame(options = {}) {
	const rows = options.rows ?? [{ id: 'todo-1', title: 'one' }];
	const position = options.position ?? '1';
	const projection = options.projection ?? 'todos-projector';
	const resume = {
		projection,
		position,
		token: options.resumeToken ?? `resume:${position}`
	};
	const records =
		options.records ??
		rows.map((row, index) => ({
			path: ['todos', String(index)],
			model: 'TodoView',
			scopeToken: options.recordScope ?? `record:${row.id}`,
			incarnation: options.incarnation ?? '1',
			revision: options.revision ?? position,
			tombstone: false
		}));
	const snapshot = {
		scopeToken: options.snapshotScope ?? 'snapshot:query',
		complete: options.complete ?? true,
		records,
		indexes:
			options.indexes ??
			(options.complete === false
				? []
				: [
						{
							projection,
							scopeToken: options.indexScope ?? 'index:query',
							position,
							resume
						}
					]),
		observations: options.observations ?? []
	};
	const live =
		options.live === undefined
			? undefined
			: {
					supported: options.live.supported ?? true,
					reset: options.live.reset ?? false,
					cursors:
						options.live.cursors ??
						(options.live.supported === false ? [] : [resume])
				};
	return {
		data: { todos: rows },
		extensions: {
			distributed: {
				protocolVersion: DISTRIBUTED_PROTOCOL_VERSION,
				schemaHash: options.schemaHash ?? 'schema-a',
				cacheScope: options.cacheScope ?? 'cache:a',
				operation: options.operation ?? 'query:todos',
				...(options.command === undefined
					? {}
					: { command: options.command }),
				snapshot,
				...(live === undefined ? {} : { live })
			}
		}
	};
}

function write(replica, options = {}, source = 'network', artifact = Todos) {
	replica.writeResult(artifact, {}, wireFrame(options), source);
}

function commandMetadata(options = {}) {
	return parseDistributedProtocolEnvelope({
		protocolVersion: DISTRIBUTED_PROTOCOL_VERSION,
		schemaHash: 'schema-a',
		cacheScope: 'cache:a',
		operation: 'command:todo',
		command: {
			commandId: options.commandId ?? 'cmd-1',
			causationId: options.causationId ?? 'cause-1',
			state: options.state ?? 'accepted_pending_projection',
			consistency: 'fact',
			expects: [
				{
					projection: 'todos-projector',
					model: 'TodoView',
					scopeToken: options.expectationToken ?? 'expect:todo-1'
				}
			],
			...(options.observations === undefined
				? {}
				: { observations: options.observations })
		}
	}).command;
}

test('v2 replica ingress rejects tampered decimals before exposing data', () => {
	const replica = createDistributedReplica();
	const tampered = wireFrame();
	tampered.extensions.distributed.snapshot.indexes[0].position = 1;

	assert.throws(
		() => replica.writeResult(Todos, {}, tampered, 'network'),
		(error) =>
			error instanceof DistributedProtocolError &&
			error.path.endsWith('.position')
	);
	assert.equal(replica.read(Todos, {}).complete, false);
	assert.equal(replica.inspectRecord(Todo, 'todo-1'), undefined);
});

test('record and index clocks reject lower or incomparable evidence without numeric coercion', () => {
	const replica = createDistributedReplica();
	write(replica, {
		position: '18446744073709551615',
		revision: '18446744073709551615',
		rows: [{ id: 'todo-1', title: 'newest' }]
	});
	write(replica, {
		position: '9',
		revision: '9',
		rows: [{ id: 'todo-1', title: 'late-old' }]
	});

	assert.equal(replica.read(Todos, {}).data.todos[0].title, 'newest');
	assert.equal(
		replica.inspectRecord(Todo, 'todo-1').revision,
		'18446744073709551615'
	);

	assert.throws(
		() =>
			write(replica, {
				position: '18446744073709551615',
				revision: '18446744073709551615',
				recordScope: 'record:incomparable',
				rows: [{ id: 'todo-1', title: 'must-not-win' }]
			}),
		DistributedProtocolError
	);
	assert.equal(
		replica.inspectRecord(Todo, 'todo-1').revision,
		'18446744073709551615'
	);
	assert.equal(replica.inspectIndex({ field: 'todos', arguments: {} }), undefined);
});

test('tombstone and explicit recreate fences reject stale resurrection', () => {
	const replica = createDistributedReplica();
	write(replica, {
		position: '1',
		revision: '1',
		rows: [{ id: 'todo-1', title: 'first lifecycle' }]
	});
	write(replica, {
		position: '9',
		rows: [],
		records: [
			{
				path: ['todos', '0'],
				model: 'TodoView',
				scopeToken: 'record:todo-1',
				incarnation: '1',
				revision: '9',
				tombstone: true
			}
		]
	});
	assert.equal(replica.inspectRecord(Todo, 'todo-1'), undefined);
	assert.deepEqual(replica.read(Todos, {}).data.todos, []);

	write(replica, {
		position: '1',
		revision: '1',
		rows: [{ id: 'todo-1', title: 'delayed pre-delete' }]
	});
	assert.equal(replica.inspectRecord(Todo, 'todo-1'), undefined);

	assert.throws(
		() =>
			write(replica, {
				position: '10',
				incarnation: '1',
				revision: '10',
				rows: [{ id: 'todo-1', title: 'implicit resurrection' }]
			}),
		DistributedProtocolError
	);
	assert.equal(replica.inspectRecord(Todo, 'todo-1'), undefined);

	write(replica, {
		position: '11',
		incarnation: '2',
		revision: '1',
		rows: [{ id: 'todo-1', title: 'explicit recreate' }]
	});
	assert.equal(replica.read(Todos, {}).data.todos[0].title, 'explicit recreate');
	assert.equal(replica.inspectRecord(Todo, 'todo-1').incarnation, '2');
	assert.equal(replica.inspectRecord(Todo, 'todo-1').revision, '1');

	write(replica, {
		position: '12',
		incarnation: '1',
		revision: '12',
		rows: [{ id: 'todo-1', title: 'stale prior lifecycle' }]
	});
	assert.equal(replica.read(Todos, {}).data.todos[0].title, 'explicit recreate');
});

test('live reset replaces its snapshot and a later query refetch may hand back', () => {
	const replica = createDistributedReplica();
	write(replica, {
		position: '5',
		rows: [{ id: 'todo-old', title: 'old snapshot' }],
		recordScope: 'record:old'
	});
	write(
		replica,
		{
			operation: 'live:todos',
			position: '6',
			rows: [{ id: 'todo-new', title: 'fresh live snapshot' }],
			recordScope: 'record:new',
			live: { reset: true }
		},
		'live'
	);
	assert.deepEqual(replica.read(Todos, {}).data.todos, [
		{ id: 'todo-new', title: 'fresh live snapshot' }
	]);

	write(replica, {
		position: '7',
		rows: [{ id: 'todo-old', title: 'later query refetch' }],
		recordScope: 'record:old'
	});
	assert.deepEqual(replica.read(Todos, {}).data.todos, [
		{ id: 'todo-old', title: 'later query refetch' }
	]);
});

test('a live handoff fences an HTTP response launched in the prior generation', async () => {
	let resolveFetch;
	let liveObserver;
	const pendingFetch = new Promise((resolve) => {
		resolveFetch = resolve;
	});
	const replica = createDistributedReplica({
		transport: {
			fetch() {
				return pendingFetch;
			},
			subscribe(_request, observer) {
				liveObserver = observer;
				return () => {};
			}
		}
	});
	const watch = replica.watch(Todos, {}, { live: true });
	await Promise.resolve();

	liveObserver.next(
		wireFrame({
			operation: 'live:todos',
			position: '1',
			rows: [{ id: 'todo-live', title: 'live wins' }],
			recordScope: 'record:live',
			live: { reset: true }
		})
	);
	resolveFetch(
		wireFrame({
			position: '99',
			rows: [{ id: 'todo-http', title: 'stale HTTP' }],
			recordScope: 'record:http'
		})
	);
	await new Promise((resolve) => setImmediate(resolve));

	assert.deepEqual(replica.read(Todos, {}).data.todos, [
		{ id: 'todo-live', title: 'live wins' }
	]);
	watch.destroy();
});

test('live advancement fences an overlapping refresh while a later clean refresh succeeds', async () => {
	const fetches = [];
	const subscriptions = [];
	let liveObserver;
	const replica = createDistributedReplica({
		transport: {
			fetch() {
				let resolve;
				const promise = new Promise((done) => {
					resolve = done;
				});
				fetches.push({ promise, resolve });
				return promise;
			},
			subscribe(request, observer) {
				subscriptions.push({ request, observer });
				liveObserver = observer;
				return () => {};
			}
		}
	});
	const watch = replica.watch(Todos, {}, { live: true });
	await Promise.resolve();
	assert.equal(fetches.length, 1);
	liveObserver.next(
		wireFrame({
			operation: 'live:todos',
			position: '1',
			rows: [{ id: 'todo-live', title: 'live one' }],
			recordScope: 'record:live',
			live: { reset: true }
		})
	);
	fetches[0].resolve(
		wireFrame({
			position: '90',
			rows: [{ id: 'todo-old-http', title: 'old HTTP' }],
			recordScope: 'record:old-http'
		})
	);
	await new Promise((resolve) => setImmediate(resolve));

	const overlapping = watch.refresh();
	await Promise.resolve();
	assert.equal(fetches.length, 2);
	liveObserver.next(
		wireFrame({
			operation: 'live:todos',
			position: '2',
			revision: '2',
			rows: [{ id: 'todo-live', title: 'live two' }],
			recordScope: 'record:live',
			live: { reset: false }
		})
	);
	fetches[1].resolve(
		wireFrame({
			position: '99',
			rows: [{ id: 'todo-racing-http', title: 'racing HTTP' }],
			recordScope: 'record:racing-http'
		})
	);
	await overlapping;
	assert.deepEqual(replica.read(Todos, {}).data.todos, [
		{ id: 'todo-live', title: 'live two' }
	]);

	const clean = watch.refresh();
	const preRefreshObserver = liveObserver;
	await Promise.resolve();
	assert.equal(fetches.length, 3);
	fetches[2].resolve(
		wireFrame({
			position: '100',
			rows: [{ id: 'todo-refreshed', title: 'clean refresh' }],
			recordScope: 'record:refreshed'
		})
	);
	await clean;
	assert.deepEqual(replica.read(Todos, {}).data.todos, [
		{ id: 'todo-refreshed', title: 'clean refresh' }
	]);
	assert.equal(subscriptions.length, 2);
	assert.notEqual(liveObserver, preRefreshObserver);
	assert.deepEqual(subscriptions[1].request.resume, [
		{
			projection: 'todos-projector',
			position: '100',
			token: 'resume:100'
		}
	]);

	preRefreshObserver.next(
		wireFrame({
			operation: 'live:todos',
			position: '999',
			revision: '999',
			rows: [{ id: 'todo-live', title: 'queued old subscription' }],
			recordScope: 'record:live',
			live: { reset: false }
		})
	);
	liveObserver.next(
		wireFrame({
			operation: 'live:todos',
			position: '101',
			revision: '3',
			rows: [{ id: 'todo-live', title: 'rebased live' }],
			recordScope: 'record:live',
			live: { reset: false }
		})
	);
	assert.deepEqual(replica.read(Todos, {}).data.todos, [
		{ id: 'todo-live', title: 'rebased live' }
	]);
	watch.destroy();
});

test('HTTP handoff requires shared scope and an equal-or-newer active index vector', async () => {
	const fetches = [];
	const subscriptions = [];
	let liveObserver;
	const replica = createDistributedReplica({
		transport: {
			fetch() {
				let resolve;
				const promise = new Promise((done) => {
					resolve = done;
				});
				fetches.push({ resolve });
				return promise;
			},
			subscribe(request, observer) {
				subscriptions.push(request);
				liveObserver = observer;
				return () => {};
			}
		}
	});
	write(replica, {
		position: '5',
		rows: [{ id: 'todo-query', title: 'query five' }],
		recordScope: 'record:query'
	});
	const watch = replica.watch(Todos, {}, { live: true });
	liveObserver.next(
		wireFrame({
			operation: 'live:todos',
			position: '10',
			rows: [{ id: 'todo-live', title: 'live ten' }],
			recordScope: 'record:live',
			live: { reset: true }
		})
	);

	const lagging = watch.refresh();
	await Promise.resolve();
	fetches[0].resolve(
		wireFrame({
			position: '9',
			rows: [{ id: 'todo-lagging', title: 'lagging HTTP' }],
			recordScope: 'record:lagging'
		})
	);
	await lagging;
	assert.deepEqual(replica.read(Todos, {}).data.todos, [
		{ id: 'todo-live', title: 'live ten' }
	]);
	assert.equal(subscriptions.length, 1);

	const incomparable = watch.refresh();
	await Promise.resolve();
	fetches[1].resolve(
		wireFrame({
			position: '11',
			snapshotScope: 'snapshot:other',
			indexScope: 'index:other',
			rows: [{ id: 'todo-other', title: 'incomparable HTTP' }],
			recordScope: 'record:other'
		})
	);
	await incomparable;
	assert.deepEqual(replica.read(Todos, {}).data.todos, [
		{ id: 'todo-live', title: 'live ten' }
	]);
	assert.equal(subscriptions.length, 1);

	const newer = watch.refresh();
	await Promise.resolve();
	fetches[2].resolve(
		wireFrame({
			position: '11',
			rows: [{ id: 'todo-newer', title: 'newer HTTP' }],
			recordScope: 'record:newer'
		})
	);
	await newer;
	assert.deepEqual(replica.read(Todos, {}).data.todos, [
		{ id: 'todo-newer', title: 'newer HTTP' }
	]);
	assert.equal(subscriptions.length, 2);
	watch.destroy();
});

test('scope and schema generations purge old state before accepting fresh evidence', () => {
	const replica = createDistributedReplica();
	write(replica, {
		rows: [{ id: 'todo-1', title: 'scope a' }],
		recordScope: 'record:a'
	});
	write(replica, {
		cacheScope: 'cache:b',
		rows: [{ id: 'todo-1', title: 'scope b' }],
		recordScope: 'record:b'
	});
	assert.equal(replica.read(Todos, {}).data.todos[0].title, 'scope b');

	assert.throws(
		() =>
			write(replica, {
				schemaHash: 'schema-tampered',
				cacheScope: 'cache:b',
				rows: [{ id: 'todo-1', title: 'wrong schema' }]
			}),
		DistributedProtocolError
	);
	assert.equal(replica.read(Todos, {}).complete, false);
});

test('protocol-bound artifacts reject results without a v2 envelope', () => {
	const replica = createDistributedReplica();
	assert.throws(
		() =>
			replica.writeResult(
				Todos,
				{},
				{
					data: { todos: [{ id: 'todo-1', title: 'unscoped' }] },
					revision: '99'
				},
				'network'
			),
		(error) =>
			error instanceof DistributedProtocolError &&
			error.path === 'extensions.distributed'
	);
	assert.equal(replica.read(Todos, {}).complete, false);
});

test('only exact causation and expectation observations retire optimism', () => {
	const replica = createDistributedReplica();
	write(replica, {
		position: '1',
		revision: '1',
		rows: [{ id: 'todo-1', title: 'base' }]
	});
	replica.createOptimisticLayer('cmd-1', (writer) => {
		writer.writeRecord(Todo, 'todo-1', {
			fields: { title: 'optimistic' }
		});
	});
	replica.markOptimisticLayerAccepted('cmd-1', commandMetadata());

	write(replica, {
		position: '2',
		revision: '2',
		rows: [{ id: 'todo-1', title: 'server before observation' }],
		observations: [
			{
				causationId: 'cause-1',
				projection: 'todos-projector',
				model: 'TodoView',
				scopeToken: 'expect:other-record'
			}
		]
	});
	assert.equal(replica.read(Todos, {}).data.todos[0].title, 'optimistic');

	write(replica, {
		position: '3',
		revision: '3',
		rows: [{ id: 'todo-1', title: 'projected' }],
		observations: [
			{
				causationId: 'cause-1',
				projection: 'todos-projector',
				model: 'TodoView',
				scopeToken: 'expect:todo-1'
			}
		]
	});
	assert.equal(replica.read(Todos, {}).data.todos[0].title, 'projected');
});

test('discarded or incomplete snapshots cannot use observations to retire optimism', () => {
	const replica = createDistributedReplica();
	write(replica, {
		position: '1',
		revision: '1',
		rows: [{ id: 'todo-1', title: 'base' }]
	});
	replica.createOptimisticLayer('cmd-1', (writer) => {
		writer.writeRecord(Todo, 'todo-1', {
			fields: { title: 'optimistic' }
		});
	});
	replica.markOptimisticLayerAccepted('cmd-1', commandMetadata());
	const observation = [
		{
			causationId: 'cause-1',
			projection: 'todos-projector',
			model: 'TodoView',
			scopeToken: 'expect:todo-1'
		}
	];

	write(replica, {
		position: '2',
		revision: '2',
		snapshotScope: 'snapshot:incomparable',
		indexScope: 'index:incomparable',
		rows: [{ id: 'todo-1', title: 'incomparable base' }],
		command: commandMetadata({ observations: observation })
	});
	write(replica, {
		position: '3',
		revision: '3',
		snapshotScope: 'snapshot:incomparable',
		indexScope: 'index:incomparable',
		rows: [{ id: 'todo-1', title: 'canonical after discard' }]
	});
	assert.equal(
		replica.read(Todos, {}).data.todos[0].title,
		'canonical after discard'
	);

	replica.createOptimisticLayer('cmd-2', (writer) => {
		writer.writeRecord(Todo, 'todo-1', {
			fields: { title: 'optimistic incomplete' }
		});
	});
	const secondReceipt = commandMetadata({
		commandId: 'cmd-2',
		causationId: 'cause-2'
	});
	replica.markOptimisticLayerAccepted('cmd-2', secondReceipt);
	const secondObservation = [
		{
			causationId: 'cause-2',
			projection: 'todos-projector',
			model: 'TodoView',
			scopeToken: 'expect:todo-1'
		}
	];

	write(replica, {
		position: '4',
		revision: '4',
		complete: false,
		rows: [{ id: 'todo-1', title: 'incomplete base' }],
		command: commandMetadata({
			commandId: 'cmd-2',
			causationId: 'cause-2',
			observations: secondObservation
		})
	});
	assert.throws(
		() => replica.createOptimisticLayer('cmd-2', () => {}),
		/optimistic layer already exists/,
		'incomplete command observation must retain its optimistic layer'
	);
	write(replica, {
		position: '5',
		revision: '5',
		snapshotScope: 'snapshot:incomparable',
		indexScope: 'index:incomparable',
		rows: [{ id: 'todo-1', title: 'canonical after incomplete' }]
	});
	assert.equal(
		replica.read(Todos, {}).data.todos[0].title,
		'canonical after incomplete'
	);

	replica.createOptimisticLayer('cmd-3', (writer) => {
		writer.writeRecord(Todo, 'todo-1', {
			fields: { title: 'optimistic snapshot observation' }
		});
	});
	replica.markOptimisticLayerAccepted(
		'cmd-3',
		commandMetadata({ commandId: 'cmd-3', causationId: 'cause-3' })
	);
	const thirdObservation = [
		{
			causationId: 'cause-3',
			projection: 'todos-projector',
			model: 'TodoView',
			scopeToken: 'expect:todo-1'
		}
	];

	write(replica, {
		position: '6',
		revision: '6',
		complete: false,
		rows: [{ id: 'todo-1', title: 'incomplete snapshot observation' }],
		observations: thirdObservation
	});
	write(replica, {
		position: '7',
		revision: '7',
		snapshotScope: 'snapshot:incomparable',
		indexScope: 'index:incomparable',
		rows: [{ id: 'todo-1', title: 'clean without observation' }]
	});
	assert.equal(
		replica.read(Todos, {}).data.todos[0].title,
		'optimistic snapshot observation'
	);
	write(replica, {
		position: '8',
		revision: '8',
		snapshotScope: 'snapshot:incomparable',
		indexScope: 'index:incomparable',
		rows: [{ id: 'todo-1', title: 'canonical base' }],
		observations: thirdObservation
	});
	assert.equal(replica.read(Todos, {}).data.todos[0].title, 'canonical base');
});

test('pathless upserts advance the global fence without certifying stale fields', () => {
	const replica = createDistributedReplica();
	write(replica, {
		position: '1',
		revision: '1',
		rows: [{ id: 'todo-1', title: 'cached one' }]
	});

	for (const revision of ['2', '3']) {
		write(replica, {
			position: revision,
			complete: false,
			rows: [],
			records: [
				{
					model: 'TodoView',
					scopeToken: 'record:todo-1',
					incarnation: '1',
					revision,
					tombstone: false
				}
			]
		});
		assert.equal(replica.inspectRecord(Todo, 'todo-1'), undefined);
	}

	write(
		replica,
		{
			operation: 'query:todos-other',
			position: '2',
			revision: '2',
			rows: [{ id: 'todo-1', title: 'late other operation' }]
		},
		'network',
		TodosOtherOperation
	);
	assert.equal(replica.inspectRecord(Todo, 'todo-1'), undefined);
});

test('an unseen pathless delete fences delayed identity discovery until recreation', () => {
	const replica = createDistributedReplica();
	write(replica, {
		position: '9',
		complete: false,
		rows: [],
		records: [
			{
				model: 'TodoView',
				scopeToken: 'record:unseen',
				incarnation: '1',
				revision: '9',
				tombstone: true
			}
		]
	});

	write(
		replica,
		{
			operation: 'query:todos-other',
			position: '1',
			incarnation: '1',
			revision: '1',
			recordScope: 'record:unseen',
			rows: [{ id: 'todo-unseen', title: 'delayed before delete' }]
		},
		'network',
		TodosOtherOperation
	);
	assert.equal(replica.inspectRecord(Todo, 'todo-unseen'), undefined);
	assert.equal(replica.read(TodosOtherOperation, {}).complete, false);

	write(
		replica,
		{
			operation: 'query:todos-other',
			position: '2',
			incarnation: '2',
			revision: '1',
			recordScope: 'record:unseen',
			rows: [{ id: 'todo-unseen', title: 'recreated' }]
		},
		'network',
		TodosOtherOperation
	);
	assert.equal(
		replica.read(TodosOtherOperation, {}).data.todos[0].title,
		'recreated'
	);
	assert.equal(replica.inspectRecord(Todo, 'todo-unseen').incarnation, '2');
	assert.equal(replica.inspectRecord(Todo, 'todo-unseen').revision, '1');
});

test('anonymous pathless clock capacity fails closed without evicting retained fences', () => {
	const replica = createDistributedReplica();
	write(replica, {
		position: '1',
		complete: false,
		rows: [],
		records: Array.from({ length: 4_096 }, (_, index) => ({
			model: 'TodoView',
			scopeToken: `record:anonymous:${index}`,
			incarnation: '1',
			revision: '1',
			tombstone: true
		}))
	});

	assert.throws(
		() =>
			write(replica, {
				position: '2',
				complete: false,
				rows: [],
				records: [
					{
						model: 'TodoView',
						scopeToken: 'record:anonymous:overflow',
						incarnation: '1',
						revision: '2',
						tombstone: true
					}
				]
			}),
		(error) =>
			error instanceof DistributedProtocolError &&
			error.path.endsWith('.records.capacity')
	);

	write(
		replica,
		{
			operation: 'query:todos-other',
			position: '1',
			incarnation: '1',
			revision: '0',
			recordScope: 'record:anonymous:0',
			rows: [{ id: 'todo-delayed', title: 'must stay deleted' }]
		},
		'network',
		TodosOtherOperation
	);
	assert.equal(replica.inspectRecord(Todo, 'todo-delayed'), undefined);
});

test('pathless delete and recreation handle reset revisions and duplicate final evidence', () => {
	const replica = createDistributedReplica();
	write(replica, {
		position: '1',
		revision: '1',
		rows: [{ id: 'todo-1', title: 'first lifecycle' }]
	});
	write(replica, {
		position: '9',
		complete: false,
		rows: [],
		records: [
			{
				model: 'TodoView',
				scopeToken: 'record:todo-1',
				incarnation: '1',
				revision: '9',
				tombstone: true
			}
		]
	});
	assert.equal(replica.inspectRecord(Todo, 'todo-1'), undefined);

	write(replica, {
		position: '10',
		complete: false,
		rows: [],
		records: [
			{
				model: 'TodoView',
				scopeToken: 'record:todo-1',
				incarnation: '2',
				revision: '1',
				tombstone: false
			}
		]
	});
	write(replica, {
		position: '11',
		incarnation: '2',
		revision: '1',
		rows: [{ id: 'todo-1', title: 'second lifecycle' }],
		records: [
			{
				path: ['todos', '0'],
				model: 'TodoView',
				scopeToken: 'record:todo-1',
				incarnation: '2',
				revision: '1',
				tombstone: false
			},
			{
				model: 'TodoView',
				scopeToken: 'record:todo-1',
				incarnation: '2',
				revision: '1',
				tombstone: false
			}
		]
	});
	assert.equal(
		replica.read(Todos, {}).data.todos[0].title,
		'second lifecycle'
	);
	assert.equal(replica.inspectRecord(Todo, 'todo-1').incarnation, '2');
	assert.equal(replica.inspectRecord(Todo, 'todo-1').revision, '1');
});

test('replica transport resumes from the latest server-issued cursor', () => {
	const subscriptions = [];
	const replica = createDistributedReplica({
		transport: {
			async fetch() {
				throw new Error('complete cache must not refetch');
			},
			subscribe(request) {
				subscriptions.push(request);
				return () => {};
			}
		}
	});
	write(replica, {
		position: '7',
		resumeToken: 'resume:latest',
		rows: [{ id: 'todo-1', title: 'cached' }]
	});
	const watch = replica.watch(Todos, {}, { live: true });
	assert.equal(subscriptions.length, 1);
	assert.deepEqual(subscriptions[0].resume, [
		{
			projection: 'todos-projector',
			position: '7',
			token: 'resume:latest'
		}
	]);
	watch.destroy();
});

test('protocol record scopes remain opaque and never become replica identities', () => {
	const replica = createDistributedReplica();
	write(replica, {
		recordScope: 'opaque:tenant/key/partition',
		rows: [{ id: 'public-id', title: 'visible' }]
	});
	assert.equal(
		replica.inspectRecord(Todo, 'public-id').key,
		replicaRecordKey(Todo, 'public-id')
	);
	assert.equal(
		replica.inspectRecord(Todo, 'opaque:tenant/key/partition'),
		undefined
	);
});
