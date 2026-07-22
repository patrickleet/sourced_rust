import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

import {
	createDistributedReplica,
	replicaRecordKey
} from '../dist/replica/index.js';

const Todo = Object.freeze({ id: 'todo-view', identityFields: ['id'] });
const User = Object.freeze({ id: 'user-view', identityFields: ['id'] });
const Comment = Object.freeze({ id: 'comment-view', identityFields: ['id'] });
const Membership = Object.freeze({
	id: 'membership-view',
	identityFields: ['org_id', 'user_id']
});

const userSelection = Object.freeze({
	model: User,
	fields: Object.freeze([
		{ kind: 'scalar', responseKey: 'userId', field: 'id' },
		{ kind: 'scalar', responseKey: 'displayName', field: 'name' }
	])
});

const commentSelection = Object.freeze({
	model: Comment,
	fields: Object.freeze([
		{ kind: 'scalar', responseKey: 'commentId', field: 'id' },
		{ kind: 'scalar', responseKey: 'body', field: 'body' }
	])
});

const todoSelection = Object.freeze({
	model: Todo,
	fields: Object.freeze([
		{ kind: 'scalar', responseKey: 'todoId', field: 'id' },
		{ kind: 'scalar', responseKey: 'headline', field: 'title' },
		// These two entries stand for fields flattened from named and inline fragments.
		{ kind: 'scalar', responseKey: 'description', field: 'description' },
		{ kind: 'scalar', responseKey: 'state', field: 'status' }
	]),
	relationships: Object.freeze([
		{
			kind: 'relationship',
			responseKey: 'owner',
			field: 'owner',
			cardinality: 'one',
			dependencies: Object.freeze(['users']),
			selection: userSelection
		},
		{
			kind: 'relationship',
			responseKey: 'notes',
			field: 'comments',
			cardinality: 'many',
			arguments: Object.freeze({
				first: Object.freeze({ kind: 'variable', name: 'commentFirst' })
			}),
			coverage: Object.freeze({ kind: 'offset', limitArgument: 'first' }),
			dependencies: Object.freeze(['comments', 'todo_comments']),
			selection: commentSelection
		}
	])
});

const TodosOpen = Object.freeze({
	id: 'TodosOpen.v1',
	document: 'query TodosOpen($first: Int!, $commentFirst: Int!) { openTodos: todos { ... } }',
	live: Object.freeze({ id: 'TodosOpen.live.v1', document: 'subscription TodosOpenLive { ... }' }),
	roots: Object.freeze([
		{
			responseKey: 'openTodos',
			field: 'todos',
			cardinality: 'many',
			arguments: Object.freeze({
				where: Object.freeze({
					kind: 'literal',
					value: Object.freeze({ status: Object.freeze({ _eq: 'open' }) })
				}),
				first: Object.freeze({ kind: 'variable', name: 'first' })
			}),
			coverage: Object.freeze({ kind: 'offset', limitArgument: 'first' }),
			dependencies: Object.freeze(['todos']),
			selection: todoSelection
		}
	])
});

const TodoByPk = Object.freeze({
	id: 'TodoByPk.v1',
	document: 'query TodoByPk($id: ID!) { item: todo_by_pk(id: $id) { ... } }',
	roots: Object.freeze([
		{
			responseKey: 'item',
			field: 'todo_by_pk',
			cardinality: 'one',
			arguments: Object.freeze({ id: Object.freeze({ kind: 'variable', name: 'id' }) }),
			dependencies: Object.freeze(['todos']),
			selection: Object.freeze({
				model: Todo,
				fields: Object.freeze([
					{ kind: 'scalar', responseKey: 'key', field: 'id' },
					{ kind: 'scalar', responseKey: 'label', field: 'title' },
					{ kind: 'scalar', responseKey: 'state', field: 'status' }
				])
			})
		}
	])
});

const Board = Object.freeze({ id: 'board-view', identityFields: ['id'] });
const Card = Object.freeze({ id: 'card-view', identityFields: ['id'] });
const cardSelection = Object.freeze({
	model: Card,
	fields: Object.freeze([
		{ kind: 'scalar', responseKey: 'cardId', field: 'id' },
		{ kind: 'scalar', responseKey: 'label', field: 'label' }
	])
});
const boardFields = Object.freeze([
	{ kind: 'scalar', responseKey: 'boardId', field: 'id' },
	{ kind: 'scalar', responseKey: 'name', field: 'name' },
	{ kind: 'scalar', responseKey: 'rowVersion', field: '__row_version', expose: false },
	{ kind: 'scalar', responseKey: 'generation', field: '__generation', expose: false }
]);
const boardRoot = Object.freeze({
	responseKey: 'board',
	field: 'board',
	cardinality: 'one',
	arguments: Object.freeze({
		id: Object.freeze({ kind: 'literal', value: 'board-1' })
	}),
	dependencies: Object.freeze(['boards']),
	selection: Object.freeze({
		model: Board,
		fields: boardFields,
		revisionResponseKey: 'rowVersion',
		incarnationResponseKey: 'generation',
		relationships: Object.freeze([
			{
				kind: 'relationship',
				responseKey: 'firstCards',
				field: 'cards',
				cardinality: 'many',
				arguments: Object.freeze({
					first: Object.freeze({ kind: 'literal', value: 1 })
				}),
				dependencies: Object.freeze(['cards']),
				selection: cardSelection
			},
			{
				kind: 'relationship',
				responseKey: 'twoCards',
				field: 'cards',
				cardinality: 'many',
				arguments: Object.freeze({
					first: Object.freeze({ kind: 'literal', value: 2 })
				}),
				dependencies: Object.freeze(['cards']),
				selection: cardSelection
			}
		])
	})
});
const BoardCards = Object.freeze({
	id: 'BoardCards.v1',
	document: 'query BoardCards { board { firstCards: cards(first: 1) { ... } } }',
	roots: Object.freeze([boardRoot])
});
const BoardRowOnly = Object.freeze({
	id: 'BoardRowOnly.v1',
	document: 'query BoardRowOnly { board { id name rowVersion generation } }',
	roots: Object.freeze([
		Object.freeze({
			...boardRoot,
			selection: Object.freeze({
				model: Board,
				fields: boardFields,
				revisionResponseKey: 'rowVersion',
				incarnationResponseKey: 'generation'
			})
		})
	])
});

const vars = Object.freeze({ first: 20, commentFirst: 10 });

function todo(id, title, options = {}) {
	return {
		todoId: id,
		headline: title,
		description: options.description ?? null,
		state: options.status ?? 'open',
		owner: options.owner ?? { userId: 'user-1', displayName: 'Ada' },
		notes: options.notes ?? [{ commentId: `comment-${id}`, body: `note for ${id}` }]
	};
}

function writeTodos(replica, revision, todos, source = 'network', errors = []) {
	replica.writeResult(
		TodosOpen,
		vars,
		{ revision, data: { openTodos: todos }, errors },
		source
	);
}

function card(id) {
	return { cardId: id, label: id.toUpperCase() };
}

function boardPayload(rowVersion, firstIds, twoIds, generation = 1) {
	return {
		boardId: 'board-1',
		name: `Board ${generation}`,
		rowVersion,
		generation,
		firstCards: firstIds.map(card),
		twoCards: twoIds.map(card)
	};
}

function boardTarget() {
	return { field: 'board', arguments: { id: 'board-1' } };
}

function rootTarget(variables = vars) {
	return {
		field: 'todos',
		arguments: {
			where: { status: { _eq: 'open' } },
			first: variables.first
		}
	};
}

class FakeTransport {
	fetches = [];
	subscriptions = [];
	unsubscribes = 0;
	#pending = [];

	fetch = (request) => {
		this.fetches.push(request);
		return new Promise((resolve, reject) => this.#pending.push({ resolve, reject }));
	};

	subscribe = (request, observer) => {
		const entry = { request, observer, active: true };
		this.subscriptions.push(entry);
		return () => {
			if (!entry.active) return;
			entry.active = false;
			this.unsubscribes += 1;
		};
	};

	resolve(index, result) {
		this.#pending[index].resolve(result);
	}

	reject(index, error) {
		this.#pending[index].reject(error);
	}

	push(index, result) {
		this.subscriptions[index].observer.next(result);
	}
}

async function flushMicrotasks() {
	await Promise.resolve();
	await Promise.resolve();
	await new Promise((resolve) => setImmediate(resolve));
}

test('normalizes aliases and flattened fragments into sparse records and nested indexes', () => {
	const replica = createDistributedReplica();
	writeTodos(replica, 1, [todo('todo-1', 'first', { description: null })], 'ssr');

	const snapshot = replica.read(TodosOpen, vars);
	assert.equal(snapshot.status, 'ready');
	assert.deepEqual(snapshot.data, {
		openTodos: [
			{
				todoId: 'todo-1',
				headline: 'first',
				description: null,
				state: 'open',
				owner: { userId: 'user-1', displayName: 'Ada' },
				notes: [{ commentId: 'comment-todo-1', body: 'note for todo-1' }]
			}
		]
	});

	const record = replica.inspectRecord(Todo, 'todo-1');
	assert.deepEqual(record.presentFields, ['description', 'id', 'status', 'title']);
	assert.equal(record.revision, '1');
	assert.equal(record.incarnation, '1');
	assert.equal(Object.hasOwn(snapshot.data.openTodos[0], 'description'), true);

	const root = replica.inspectIndex(rootTarget());
	assert.deepEqual(root.arguments, { where: { status: { _eq: 'open' } }, first: 20 });
	assert.deepEqual(root.dependencies, ['todos']);
	assert.deepEqual(root.coverage, { kind: 'offset', offset: 0, limit: 20, returned: 1 });
	assert.equal(root.complete, true);
	assert.equal(root.staleReason, undefined);

	const todoKey = replicaRecordKey(Todo, 'todo-1');
	const comments = replica.inspectIndex({
		parent: todoKey,
		field: 'comments',
		arguments: { first: 10 }
	});
	assert.deepEqual(comments.dependencies, ['comments', 'todo_comments']);
	assert.deepEqual(comments.coverage, {
		kind: 'offset',
		offset: 0,
		limit: 10,
		returned: 1
	});
	assert.equal(comments.records.length, 1);
});

test('missing differs from null and partial GraphQL errors never certify errored paths', () => {
	const replica = createDistributedReplica();
	writeTodos(
		replica,
		1,
		[
			{
				todoId: 'todo-2',
				headline: null,
				state: 'open',
				owner: { userId: 'user-1', displayName: 'Ada' },
				notes: []
			}
		],
		'network',
		[{ message: 'description failed', path: ['openTodos', 0, 'description'] }]
	);

	const record = replica.inspectRecord(Todo, 'todo-2');
	assert.equal(record.presentFields.includes('title'), true);
	assert.equal(record.presentFields.includes('description'), false);
	const snapshot = replica.read(TodosOpen, vars);
	assert.equal(snapshot.data.openTodos[0].headline, null);
	assert.equal(Object.hasOwn(snapshot.data.openTodos[0], 'headline'), true);
	assert.equal(Object.hasOwn(snapshot.data.openTodos[0], 'description'), false);
	assert.equal(snapshot.complete, false);
	assert.equal(snapshot.stale, true);
	assert.equal(snapshot.status, 'error');
	assert.equal(replica.inspectIndex(rootTarget()).staleReason, 'graphql-partial-error');
});

test('a same-revision retry can monotonically refine a partial index to complete', () => {
	const replica = createDistributedReplica();
	writeTodos(
		replica,
		10,
		[todo('todo-1', 'one'), null],
		'network',
		[{ message: 'second row failed', path: ['openTodos', 1, 'headline'] }]
	);
	let snapshot = replica.read(TodosOpen, vars);
	assert.equal(snapshot.complete, false);
	assert.equal(snapshot.stale, true);
	assert.deepEqual(snapshot.data.openTodos.map((entry) => entry.todoId), ['todo-1']);

	assert.doesNotThrow(() =>
		writeTodos(replica, 10, [todo('todo-1', 'one'), todo('todo-2', 'two')])
	);
	snapshot = replica.read(TodosOpen, vars);
	assert.equal(snapshot.status, 'ready');
	assert.equal(snapshot.complete, true);
	assert.deepEqual(
		snapshot.data.openTodos.map((entry) => entry.todoId),
		['todo-1', 'todo-2']
	);
});

test('same-revision partial refinement rejects contradictory known membership', () => {
	const replica = createDistributedReplica();
	writeTodos(
		replica,
		10,
		[todo('todo-1', 'one'), null, todo('todo-3', 'three')],
		'network',
		[{ message: 'middle row failed', path: ['openTodos', 1, 'headline'] }]
	);
	assert.throws(
		() =>
			writeTodos(replica, 10, [todo('todo-3', 'three'), todo('todo-1', 'one')]),
		/conflicting cache values/
	);
	const snapshot = replica.read(TodosOpen, vars);
	assert.equal(snapshot.stale, true);
	assert.deepEqual(
		snapshot.data.openTodos.map((entry) => entry.todoId),
		['todo-1', 'todo-3']
	);
});

test('a root-level partial error preserves available data and does not refetch-loop', async () => {
	const transport = new FakeTransport();
	const replica = createDistributedReplica({ transport });
	writeTodos(replica, 1, [todo('todo-1', 'available')]);
	const watch = replica.watch(TodosOpen, vars);
	assert.equal(transport.fetches.length, 0);

	replica.writeResult(
		TodosOpen,
		vars,
		{
			revision: 2,
			data: { openTodos: null },
			errors: [{ message: 'root null-propagated', path: ['openTodos', 0, 'headline'] }]
		},
		'live'
	);
	await flushMicrotasks();
	assert.equal(watch.get().data.openTodos[0].headline, 'available');
	assert.equal(watch.get().stale, true);
	assert.equal(transport.fetches.length, 1);

	transport.resolve(0, {
		revision: 3,
		data: { openTodos: [todo('todo-1', 'still partial')] },
		errors: [{ message: 'title failed', path: ['openTodos', 0, 'headline'] }]
	});
	await flushMicrotasks();
	assert.equal(transport.fetches.length, 1, 'partial settlement does not spin a request loop');
	assert.equal(watch.get().data.openTodos[0].headline, 'available');
	watch.destroy();
});

test('exact indexes distinguish canonical arguments, explicit null, omission, and parents', () => {
	const replica = createDistributedReplica();
	writeTodos(replica, 1, [todo('todo-1', 'first')]);
	const reordered = replica.inspectIndex({
		field: 'todos',
		arguments: { first: 20, where: { status: { _eq: 'open' } } }
	});
	assert.ok(reordered);
	assert.equal(replica.inspectIndex({ field: 'todos', arguments: { first: null } }), undefined);
	assert.equal(replica.inspectIndex({ field: 'todos', arguments: {} }), undefined);

	const todoKey = replicaRecordKey(Todo, 'todo-1');
	assert.ok(
		replica.inspectIndex({ parent: todoKey, field: 'comments', arguments: { first: 10 } })
	);
	assert.equal(
		replica.inspectIndex({
			parent: replicaRecordKey(Todo, 'todo-other'),
			field: 'comments',
			arguments: { first: 10 }
		}),
		undefined
	);
	assert.notEqual(
		replicaRecordKey(Membership, ['org-1', 'user-1']),
		replicaRecordKey(Membership, ['org-1', 'user-2'])
	);
});

test('argument-distinct relationship aliases use the response checkpoint, not the parent row clock', () => {
	const replica = createDistributedReplica();
	replica.writeResult(
		BoardCards,
		{},
		{
			revision: 10,
			data: { board: boardPayload(1, ['a'], ['a', 'b']) }
		},
		'network'
	);
	assert.deepEqual(replica.read(BoardCards, {}).data, {
		board: {
			boardId: 'board-1',
			name: 'Board 1',
			firstCards: [{ cardId: 'a', label: 'A' }],
			twoCards: [
				{ cardId: 'a', label: 'A' },
				{ cardId: 'b', label: 'B' }
			]
		}
	});

	// Relationship membership advances at the response checkpoint while the
	// SQL row itself legitimately remains at revision 1.
	replica.writeResult(
		BoardCards,
		{},
		{
			revision: 11,
			data: { board: boardPayload(1, ['c'], ['c', 'd']) }
		},
		'live'
	);
	const snapshot = replica.read(BoardCards, {});
	assert.equal(snapshot.status, 'ready');
	assert.deepEqual(
		snapshot.data.board.firstCards.map((entry) => entry.cardId),
		['c']
	);
	assert.deepEqual(
		snapshot.data.board.twoCards.map((entry) => entry.cardId),
		['c', 'd']
	);
	const parent = replicaRecordKey(Board, 'board-1');
	assert.equal(
		replica.inspectIndex({ parent, field: 'cards', arguments: { first: 1 } }).revision,
		'11'
	);
	assert.equal(
		replica.inspectIndex({ parent, field: 'cards', arguments: { first: 2 } }).revision,
		'11'
	);
});

test('partial wire-clock errors and top-level null preserve last-known-good data', () => {
	const replica = createDistributedReplica();
	replica.writeResult(
		BoardCards,
		{},
		{ revision: 10, data: { board: boardPayload(1, ['a'], ['a', 'b']) } },
		'network'
	);
	const before = replica.read(BoardCards, {}).data;
	const errored = boardPayload(1, ['changed'], ['changed']);
	errored.rowVersion = null;
	assert.doesNotThrow(() =>
		replica.writeResult(
			BoardCards,
			{},
			{
				revision: 11,
				data: { board: errored },
				errors: [{ message: 'row clock unavailable', path: ['board', 'rowVersion'] }]
			},
			'live'
		)
	);
	let snapshot = replica.read(BoardCards, {});
	assert.equal(snapshot.status, 'error');
	assert.equal(snapshot.stale, true);
	assert.deepEqual(snapshot.data, before);

	assert.doesNotThrow(() =>
		replica.writeResult(
			BoardCards,
			{},
			{
				revision: 12,
				data: null,
				errors: [{ message: 'non-null propagation', path: ['board', 'name'] }]
			},
			'network'
		)
	);
	snapshot = replica.read(BoardCards, {});
	assert.deepEqual(snapshot.data, before);
	assert.equal(snapshot.status, 'error');
	assert.equal(replica.inspectIndex(boardTarget()).staleRevision, '12');
});

test('a rejected parent revision makes the operation stale instead of silently ready', () => {
	const replica = createDistributedReplica();
	replica.writeResult(
		BoardCards,
		{},
		{ revision: 5, data: { board: boardPayload(1, ['a'], ['a']) } },
		'network'
	);
	replica.writeResult(
		BoardRowOnly,
		{},
		{
			revision: 9,
			data: {
				board: {
					boardId: 'board-1',
					name: 'Board 1',
					rowVersion: 2,
					generation: 1
				}
			}
		},
		'projected'
	);
	assert.doesNotThrow(
		() =>
			replica.writeResult(
				BoardCards,
				{},
				{ revision: 10, data: { board: boardPayload(1, ['b'], ['b']) } },
				'network'
			)
	);
	const snapshot = replica.read(BoardCards, {});
	assert.equal(snapshot.status, 'stale');
	assert.equal(snapshot.complete, false);
	assert.equal(Object.hasOwn(snapshot.data, 'board'), false);
	assert.equal(replica.inspectIndex(boardTarget()).staleReason, 'incomplete-result');
});

test('older partial results cannot stale newer data or replace its query errors', () => {
	const replica = createDistributedReplica();
	writeTodos(replica, 10, [todo('todo-1', 'newest')]);
	replica.writeResult(
		TodosOpen,
		vars,
		{
			revision: 9,
			data: { openTodos: null },
			errors: [{ message: 'late failure', path: ['openTodos', 0, 'headline'] }]
		},
		'live'
	);
	const snapshot = replica.read(TodosOpen, vars);
	assert.equal(snapshot.status, 'ready');
	assert.equal(snapshot.data.openTodos[0].headline, 'newest');
	assert.deepEqual(snapshot.errors, []);
	assert.equal(replica.inspectIndex(rootTarget()).staleReason, undefined);
});

test('an uncached error checkpoint fences delayed data from another artifact', () => {
	const replica = createDistributedReplica();
	const sameRootOtherArtifact = Object.freeze({
		...TodosOpen,
		id: 'TodosOpen.other-artifact.v1'
	});
	replica.writeResult(
		TodosOpen,
		vars,
		{
			revision: 10,
			data: null,
			errors: [{ message: 'root failed', path: ['openTodos'] }]
		},
		'network'
	);
	assert.equal(replica.inspectIndex(rootTarget()), undefined);

	replica.writeResult(
		sameRootOtherArtifact,
		vars,
		{ revision: 9, data: { openTodos: [todo('todo-1', 'delayed')] } },
		'live'
	);
	let snapshot = replica.read(sameRootOtherArtifact, vars);
	assert.equal(snapshot.complete, false);
	assert.equal(snapshot.status, 'loading');
	assert.equal(Object.hasOwn(snapshot.data, 'openTodos'), false);

	replica.writeResult(
		sameRootOtherArtifact,
		vars,
		{ revision: 10, data: { openTodos: [todo('todo-1', 'checkpoint')] } },
		'network'
	);
	snapshot = replica.read(sameRootOtherArtifact, vars);
	assert.equal(snapshot.status, 'ready');
	assert.equal(snapshot.data.openTodos[0].headline, 'checkpoint');
});

test('a successful revision-zero retry replaces an uncached error fence', () => {
	const replica = createDistributedReplica();
	replica.writeResult(
		TodosOpen,
		vars,
		{
			revision: 0,
			data: null,
			errors: [{ message: 'initial failure', path: ['openTodos'] }]
		},
		'network'
	);
	replica.writeResult(
		Object.freeze({ ...TodosOpen, id: 'TodosOpen.revision-zero-retry.v1' }),
		vars,
		{ revision: 0, data: { openTodos: [todo('todo-0', 'zero checkpoint')] } },
		'network'
	);
	const snapshot = replica.read(TodosOpen, vars);
	assert.equal(snapshot.complete, true);
	assert.equal(snapshot.data.openTodos[0].headline, 'zero checkpoint');
});

test('one ingress transaction exposes no half-written nested response and rolls back failures', () => {
	const replica = createDistributedReplica();
	writeTodos(replica, 1, [todo('todo-1', 'one')], 'restore');
	const watch = replica.watch(TodosOpen, vars);
	const seen = [];
	const unsubscribe = watch.subscribe((snapshot) => seen.push(snapshot.data));
	seen.length = 0;
	writeTodos(replica, 2, [todo('todo-1', 'updated'), todo('todo-2', 'second')], 'live');
	assert.equal(seen.length, 1);
	assert.deepEqual(
		seen[0].openTodos.map((entry) => [entry.headline, entry.owner.displayName, entry.notes.length]),
		[
			['updated', 'Ada', 1],
			['second', 'Ada', 1]
		]
	);

	const broken = {
		...TodosOpen,
		id: 'BrokenAliases.v1',
		roots: [
			{
				...TodosOpen.roots[0],
				selection: {
					model: Todo,
					fields: [
						{ kind: 'scalar', responseKey: 'todoId', field: 'id' },
						{ kind: 'scalar', responseKey: 'headline', field: 'title' },
						{ kind: 'scalar', responseKey: 'otherTitle', field: 'title' }
					]
				}
			}
		]
	};
	assert.throws(
		() =>
			replica.writeResult(
				broken,
				vars,
				{
					revision: 3,
					data: {
						openTodos: [{ todoId: 'todo-3', headline: 'left', otherTitle: 'right' }]
					}
				},
				'network'
			),
		/aliases disagree/
	);
	assert.equal(replica.inspectRecord(Todo, 'todo-3'), undefined);
	assert.equal(watch.get().data.openTodos.length, 2);
	unsubscribe();
	watch.destroy();
});

test('cache-and-live fetches missing data once, deduplicates work, and detaches the last watch', async () => {
	const transport = new FakeTransport();
	const replica = createDistributedReplica({ transport });
	const first = replica.watch(TodosOpen, vars, { live: true });
	const second = replica.watch(TodosOpen, { commentFirst: 10, first: 20 }, { live: true });
	await flushMicrotasks();
	assert.equal(transport.fetches.length, 1);
	assert.equal(transport.subscriptions.length, 1);
	assert.equal(first.get().fetching, true);
	assert.equal(first.get().live, 'active');

	transport.resolve(0, { revision: 1, data: { openTodos: [todo('todo-1', 'fetched')] } });
	await flushMicrotasks();
	assert.equal(first.get().status, 'ready');
	assert.equal(first.get().fetching, false);
	assert.equal(second.get().data.openTodos[0].headline, 'fetched');

	transport.push(0, {
		revision: 2,
		data: { openTodos: [todo('todo-1', 'live update')] }
	});
	assert.equal(first.get().data.openTodos[0].headline, 'live update');
	assert.equal(transport.fetches.length, 1);

	first.destroy();
	assert.equal(transport.unsubscribes, 0);
	second.destroy();
	assert.equal(transport.unsubscribes, 1);
});

test('a synchronous live terminal error stays terminal and runs returned cleanup', () => {
	let cleanups = 0;
	const transport = {
		fetch: () => Promise.reject(new Error('complete cache must not fetch')),
		subscribe: (_request, observer) => {
			observer.error(new Error('synchronous live failure'));
			return () => {
				cleanups += 1;
			};
		}
	};
	const replica = createDistributedReplica({ transport });
	writeTodos(replica, 1, [todo('todo-1', 'cached')]);
	const watch = replica.watch(TodosOpen, vars, { live: true });
	assert.equal(watch.get().live, 'error');
	assert.equal(watch.get().status, 'error');
	assert.match(watch.get().errors[0].message, /synchronous live failure/);
	assert.equal(cleanups, 1);
	watch.destroy();
	assert.equal(cleanups, 1);
});

test('a failing immediate replica subscriber is isolated and removed', () => {
	const reported = [];
	const replica = createDistributedReplica({ onObserverError: (error) => reported.push(error) });
	writeTodos(replica, 1, [todo('todo-1', 'initial')]);
	const watch = replica.watch(TodosOpen, vars);
	let calls = 0;
	assert.doesNotThrow(() =>
		watch.subscribe(() => {
			calls += 1;
			throw new Error('subscriber failed');
		})
	);
	assert.equal(calls, 1);
	assert.equal(reported.length, 1);
	writeTodos(replica, 2, [todo('todo-1', 'later')]);
	assert.equal(calls, 1);
	watch.destroy();
});

test('published GraphQL errors are deeply detached and frozen', () => {
	const path = ['openTodos', 0, 'description'];
	const extensions = { code: 'READ_FAILED', detail: { retryable: true } };
	const locations = [{ line: 2, column: 3 }];
	const replica = createDistributedReplica();
	writeTodos(
		replica,
		1,
		[todo('todo-1', 'one')],
		'network',
		[{ message: 'field failed', path, extensions, locations }]
	);
	path[0] = 'mutated';
	extensions.detail.retryable = false;
	locations[0].line = 99;
	const [published] = replica.read(TodosOpen, vars).errors;
	assert.deepEqual(published.path, ['openTodos', 0, 'description']);
	assert.deepEqual(published.extensions, {
		code: 'READ_FAILED',
		detail: { retryable: true }
	});
	assert.deepEqual(published.locations, [{ line: 2, column: 3 }]);
	assert.equal(Object.isFrozen(published.extensions.detail), true);
});

test('complete cache skips HTTP, stale cache refetches, and explicit refresh deduplicates', async () => {
	const transport = new FakeTransport();
	const replica = createDistributedReplica({ transport });
	writeTodos(replica, 1, [todo('todo-1', 'cached')]);
	const complete = replica.watch(TodosOpen, vars, { live: true });
	await flushMicrotasks();
	assert.equal(transport.fetches.length, 0);
	assert.equal(transport.subscriptions.length, 1);

	replica.markIndexStale(rootTarget(), 'manual-test-stale');
	await flushMicrotasks();
	assert.equal(transport.fetches.length, 1);
	assert.equal(complete.get().stale, true);
	const firstRefresh = complete.refresh();
	const secondRefresh = complete.refresh();
	assert.equal(transport.fetches.length, 1);
	transport.resolve(0, { revision: 2, data: { openTodos: [todo('todo-1', 'fresh')] } });
	await Promise.all([firstRefresh, secondRefresh]);
	assert.equal(complete.get().status, 'ready');
	assert.equal(complete.get().data.openTodos[0].headline, 'fresh');
	complete.destroy();
});

test('exact selection dependencies preserve identity for unrelated writes', () => {
	const replica = createDistributedReplica();
	writeTodos(replica, 1, [todo('todo-1', 'one'), todo('todo-2', 'two')]);
	replica.writeResult(
		TodoByPk,
		{ id: 'todo-1' },
		{
			revision: 1,
			data: { item: { key: 'todo-1', label: 'one', state: 'open' } }
		},
		'network'
	);
	const list = replica.watch(TodosOpen, vars);
	const byPk = replica.watch(TodoByPk, { id: 'todo-1' });
	const beforeList = list.get().data;
	const beforeByPk = byPk.get().data;
	let listCalls = 0;
	let byPkCalls = 0;
	list.subscribe(() => listCalls++);
	byPk.subscribe(() => byPkCalls++);
	listCalls = 0;
	byPkCalls = 0;

	replica.createOptimisticLayer('unselected', (writer) => {
		writer.writeRecord(Todo, 'todo-1', { fields: { internal_note: 'not selected' } });
	});
	assert.equal(listCalls, 0);
	assert.equal(byPkCalls, 0);
	assert.equal(list.get().data, beforeList);
	assert.equal(byPk.get().data, beforeByPk);
	replica.rejectOptimisticLayer('unselected');

	replica.createOptimisticLayer('rename', (writer) => {
		writer.writeRecord(Todo, 'todo-1', { fields: { title: 'renamed' } });
	});
	assert.equal(listCalls, 1);
	assert.equal(byPkCalls, 1);
	assert.equal(list.get().data.openTodos[0].headline, 'renamed');
	assert.equal(byPk.get().data.item.label, 'renamed');
	replica.rejectOptimisticLayer('rename');
	listCalls = 0;
	byPkCalls = 0;
	replica.createOptimisticLayer('reorder', (writer) => {
		writer.writeIndex(
			{
				...rootTarget(),
				complete: true,
				coverage: { kind: 'offset', offset: 0, limit: 20, returned: 2 },
				dependencies: ['todos']
			},
			[replicaRecordKey(Todo, 'todo-2'), replicaRecordKey(Todo, 'todo-1')]
		);
	});
	assert.equal(listCalls, 1);
	assert.equal(byPkCalls, 0);
	assert.deepEqual(
		list.get().data.openTodos.map((entry) => entry.todoId),
		['todo-2', 'todo-1']
	);
	replica.rejectOptimisticLayer('reorder');
	list.destroy();
	byPk.destroy();
});

test('named optimistic layers rebase independently over confirmed server state', () => {
	const replica = createDistributedReplica();
	writeTodos(replica, 1, [todo('todo-1', 'base')]);
	const watch = replica.watch(TodosOpen, vars);
	const values = [];
	watch.subscribe((snapshot) => values.push(snapshot.data.openTodos?.[0]?.headline));
	values.length = 0;

	replica.createOptimisticLayer('A', (writer) => {
		writer.writeRecord(Todo, 'todo-1', { fields: { title: 'A' } });
	});
	replica.createOptimisticLayer('B', (writer) => {
		writer.writeRecord(Todo, 'todo-1', { fields: { title: 'B' } });
	});
	assert.equal(watch.get().data.openTodos[0].headline, 'B');

	const beforeConfirm = values.length;
	replica.confirmOptimisticLayer('B', (writer) => {
		writer.writeRecord(Todo, 'todo-1', 2, { fields: { title: 'B projected' } });
	});
	assert.equal(values.length, beforeConfirm + 1, 'base write and layer removal emit once');
	assert.equal(watch.get().data.openTodos[0].headline, 'B projected');
	replica.confirmOptimisticLayer('A', (writer) => {
		assert.equal(
			writer.writeRecord(Todo, 'todo-1', 1, { fields: { title: 'A stale' } }),
			false
		);
	});
	assert.equal(watch.get().data.openTodos[0].headline, 'B projected');

	replica.createOptimisticLayer('title', (writer) => {
		writer.writeRecord(Todo, 'todo-1', { fields: { title: 'pending title' } });
	});
	replica.createOptimisticLayer('status', (writer) => {
		writer.writeRecord(Todo, 'todo-1', { fields: { status: 'completed' } });
	});
	assert.equal(watch.get().data.openTodos[0].headline, 'pending title');
	assert.equal(watch.get().data.openTodos[0].state, 'completed');
	replica.rejectOptimisticLayer('title');
	assert.equal(watch.get().data.openTodos[0].headline, 'B projected');
	assert.equal(watch.get().data.openTodos[0].state, 'completed');
	replica.rejectOptimisticLayer('status');
	watch.destroy();
});

test('revision, tombstone, equal-conflict, and incarnation fences prevent stale resurrection', () => {
	const replica = createDistributedReplica();
	writeTodos(replica, 5, [todo('todo-1', 'v5')]);
	assert.equal(replica.tombstoneRecord(Todo, 'todo-1', 6), true);
	writeTodos(replica, 4, [todo('todo-1', 'late v4')]);
	assert.equal(replica.inspectRecord(Todo, 'todo-1'), undefined);

	assert.throws(
		() => writeTodos(replica, 6, [todo('todo-1', 'equal conflict')]),
		/conflicting cache values at revision 6/
	);
	assert.equal(replica.inspectIndex(rootTarget()).staleReason, 'revision-conflict');

	writeTodos(replica, 7, [todo('todo-1', 'recreated')]);
	const recreated = replica.inspectRecord(Todo, 'todo-1');
	assert.equal(recreated.incarnation, '7');
	assert.equal(replica.read(TodosOpen, vars).data.openTodos[0].headline, 'recreated');

	assert.throws(
		() => writeTodos(replica, 7, [todo('todo-1', 'same revision disagreement')]),
		/conflicting cache values at revision 7/
	);
	assert.equal(replica.read(TodosOpen, vars).data.openTodos[0].headline, 'recreated');
});

test('recreating a parent never reuses relationship indexes from its prior incarnation', () => {
	const replica = createDistributedReplica();
	replica.writeResult(
		BoardCards,
		{},
		{ revision: 10, data: { board: boardPayload(1, ['old'], ['old']) } },
		'network'
	);
	assert.equal(replica.tombstoneRecord(Board, 'board-1', 2), true);
	replica.writeResult(
		BoardRowOnly,
		{},
		{
			revision: 11,
			data: {
				board: {
					boardId: 'board-1',
					name: 'Board 3',
					rowVersion: 3,
					generation: 3
				}
			}
		},
		'projected'
	);
	const snapshot = replica.read(BoardCards, {});
	assert.equal(snapshot.complete, false);
	assert.equal(snapshot.data.board.name, 'Board 3');
	assert.equal(Object.hasOwn(snapshot.data.board, 'firstCards'), false);
	const parent = replicaRecordKey(Board, 'board-1');
	assert.equal(
		replica.inspectIndex({ parent, field: 'cards', arguments: { first: 1 } }),
		undefined
	);
});

test('explicit incarnation fences reject resurrection of a tombstoned lifecycle', () => {
	const replica = createDistributedReplica();
	replica.writeResult(
		BoardRowOnly,
		{},
		{
			revision: 10,
			data: {
				board: {
					boardId: 'board-1',
					name: 'first lifecycle',
					rowVersion: 1,
					generation: 1
				}
			}
		},
		'network'
	);
	replica.tombstoneRecord(Board, 'board-1', 2);
	assert.throws(
		() =>
			replica.writeResult(
				BoardRowOnly,
				{},
				{
					revision: 11,
					data: {
						board: {
							boardId: 'board-1',
							name: 'invalid resurrection',
							rowVersion: 3,
							generation: 1
						}
					}
				},
				'live'
			),
		/conflicting cache values/
	);
	assert.equal(replica.inspectRecord(Board, 'board-1'), undefined);
	assert.equal(replica.read(BoardRowOnly, {}).complete, false);

	replica.writeResult(
		BoardRowOnly,
		{},
		{
			revision: 12,
			data: {
				board: {
					boardId: 'board-1',
					name: 'new lifecycle',
					rowVersion: 3,
					generation: 3
				}
			}
		},
		'live'
	);
	assert.equal(replica.read(BoardRowOnly, {}).data.board.name, 'new lifecycle');
});

test('filtered root membership never transfers to a recreated key incarnation', () => {
	const replica = createDistributedReplica();
	writeTodos(replica, 10, [todo('todo-1', 'open lifecycle')]);
	replica.tombstoneRecord(Todo, 'todo-1', 11);
	replica.writeResult(
		TodoByPk,
		{ id: 'todo-1' },
		{
			revision: 12,
			data: { item: { key: 'todo-1', label: 'closed lifecycle', state: 'closed' } }
		},
		'projected'
	);
	let snapshot = replica.read(TodosOpen, vars);
	assert.equal(snapshot.stale, true);
	assert.deepEqual(snapshot.data.openTodos, []);

	assert.throws(
		() => writeTodos(replica, 10, [todo('todo-1', 'delayed old lifecycle')]),
		/conflicting cache values/
	);
	snapshot = replica.read(TodosOpen, vars);
	assert.equal(snapshot.complete, false);
	assert.deepEqual(snapshot.data.openTodos, []);
	assert.equal(replica.inspectRecord(Todo, 'todo-1').incarnation, '12');
});

test('relationship membership never transfers a child key across incarnations', () => {
	const replica = createDistributedReplica();
	replica.writeResult(
		BoardCards,
		{},
		{ revision: 10, data: { board: boardPayload(1, ['a'], ['a']) } },
		'network'
	);
	replica.tombstoneRecord(Card, 'a', 11);
	replica.createOptimisticLayer('recreate-card', (writer) => {
		writer.writeRecord(Card, 'a', { fields: { id: 'a', label: 'NEW' } });
	});
	replica.confirmOptimisticLayer('recreate-card', (writer) => {
		writer.writeRecord(Card, 'a', 12, { fields: { id: 'a', label: 'NEW' } });
	});
	let snapshot = replica.read(BoardCards, {});
	assert.equal(snapshot.stale, true);
	assert.deepEqual(snapshot.data.board.firstCards, []);

	assert.throws(
		() =>
			replica.writeResult(
				BoardCards,
				{},
				{ revision: 10, data: { board: boardPayload(1, ['a'], ['a']) } },
				'live'
			),
		/conflicting cache values/
	);
	snapshot = replica.read(BoardCards, {});
	assert.deepEqual(snapshot.data.board.firstCards, []);
	assert.equal(replica.inspectRecord(Card, 'a').incarnation, '12');
});

test('reachability GC follows indexes and links, keeps fences, and collects true orphans', () => {
	const replica = createDistributedReplica();
	writeTodos(replica, 1, [todo('todo-1', 'reachable')]);
	replica.createOptimisticLayer('orphan-seed', (writer) => {
		writer.writeRecord(Todo, 'orphan', { fields: { id: 'orphan', title: 'optimistic' } });
	});
	replica.confirmOptimisticLayer('orphan-seed', (writer) => {
		writer.writeRecord(Todo, 'orphan', 2, {
			fields: { id: 'orphan', title: 'confirmed orphan' }
		});
	});
	replica.tombstoneRecord(Todo, 'deleted', 3);

	assert.deepEqual(replica.gc(), [replicaRecordKey(Todo, 'orphan')]);
	assert.ok(replica.inspectRecord(Todo, 'todo-1'));
	assert.ok(replica.inspectRecord(User, 'user-1'));
	assert.ok(replica.inspectRecord(Comment, 'comment-todo-1'));
	assert.equal(replica.inspectRecord(Todo, 'deleted'), undefined);
	assert.deepEqual(replica.gc(), []);
});

test('replica GC follows reachable relationship edges and collects orphan nested graphs', () => {
	const replica = createDistributedReplica();
	replica.writeResult(
		BoardCards,
		{},
		{ revision: 10, data: { board: boardPayload(1, ['a'], ['a', 'b']) } },
		'network'
	);
	assert.deepEqual(replica.gc(), []);
	replica.createOptimisticLayer('remove-board-root', (writer) => {
		writer.deleteIndex(boardTarget());
	});
	replica.confirmOptimisticLayer('remove-board-root', (writer) => {
		writer.deleteIndex(boardTarget(), 11);
	});
	assert.deepEqual(replica.gc(), [
		replicaRecordKey(Board, 'board-1'),
		replicaRecordKey(Card, 'a'),
		replicaRecordKey(Card, 'b')
	].sort());
	assert.equal(replica.inspectRecord(Board, 'board-1'), undefined);
	assert.equal(replica.inspectRecord(Card, 'a'), undefined);
	assert.equal(
		replica.inspectIndex({
			parent: replicaRecordKey(Board, 'board-1'),
			field: 'cards',
			arguments: { first: 1 }
		}),
		undefined
	);
});

test('replica entry declarations expose no private cache-engine types', async () => {
	for (const file of [
		'../dist/replica/index.d.ts',
		'../dist/replica/distributed-replica.d.ts',
		'../dist/replica/identity.d.ts',
		'../dist/replica/types.d.ts'
	]) {
		const declaration = await readFile(new URL(file, import.meta.url), 'utf8');
		assert.doesNotMatch(
			declaration,
			/internal\/cache-engine|\bCacheEngine\b|\bCacheValue\b|\bCacheIndexCoverage\b/
		);
	}
});
