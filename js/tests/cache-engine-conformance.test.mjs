import assert from 'node:assert/strict';
import { readFile, readdir } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { InMemoryCache } from '@apollo/client/cache';
import { gql } from '@apollo/client';
import {
	cacheIndexKey,
	createCacheEngine
} from '../dist/internal/cache-engine.js';

const TODO = 'Todo:todo-1';
const OTHER_TODO = 'Todo:todo-2';
const USER = 'User:user-1';
const OPEN = cacheIndexKey({ field: 'todos', arguments: { status: 'open', first: 20 } });
const OWNED_OPEN = cacheIndexKey({
	field: 'todos',
	arguments: { status: 'open', owner: 'user-1', first: 20 }
});
const ALL = cacheIndexKey({ field: 'todos', arguments: { first: 20 } });
const USER_TODOS = cacheIndexKey({ parent: USER, field: 'todos', arguments: { first: 20 } });

function todoFields(reader) {
	return reader.record(TODO)?.fields;
}

function todoAt(reader, indexKey) {
	const index = reader.index(indexKey);
	return index?.records.map((key) => reader.record(key)?.fields);
}

/** Generated-like response plan: aliases and fragments map to canonical fields. */
function normalizeAliasedTodo(writer, payload, revision) {
	writer.writeRecord({
		key: `Todo:${payload.todoId}`,
		revision,
		fields: {
			id: payload.todoId,
			title: payload.headline,
			status: payload.state,
			description: payload.description
		},
		links: { owner: `User:${payload.ownerId}` }
	});
	writer.writeRecord({
		key: `User:${payload.ownerId}`,
		revision,
		fields: { id: payload.ownerId, name: payload.ownerName },
		links: { todos: [`Todo:${payload.todoId}`] }
	});
	writer.writeIndex({ key: OPEN, revision, records: [TODO], complete: true });
	writer.writeIndex({ key: OWNED_OPEN, revision, records: [TODO], complete: true });
	writer.writeIndex({ key: ALL, revision, records: [TODO], complete: true });
	writer.writeIndex({ key: USER_TODOS, revision, records: [TODO], complete: true });
}

test('purpose-built engine normalizes sparse records, links, and exact indexes in one batch', () => {
	const engine = createCacheEngine();
	const notifications = new Map();
	const watch = (name, selector) =>
		engine.watch(selector, () => notifications.set(name, (notifications.get(name) ?? 0) + 1));

	watch('by-pk', (reader) => reader.record(TODO));
	watch('alias-fragment', (reader) => reader.record(TODO)?.fields.title);
	watch('open', (reader) => todoAt(reader, OPEN));
	watch('owned-open', (reader) => todoAt(reader, OWNED_OPEN));
	watch('all', (reader) => todoAt(reader, ALL));
	watch('relationship', (reader) => todoAt(reader, USER_TODOS));
	watch('unrelated', (reader) => reader.record(OTHER_TODO));

	engine.batch((writer) => {
		normalizeAliasedTodo(
			writer,
			{
				todoId: 'todo-1',
				headline: 'original',
				state: 'open',
				description: null,
				ownerId: 'user-1',
				ownerName: 'Ada'
			},
			1
		);
	});

	assert.deepEqual(Object.fromEntries(notifications), {
		'by-pk': 1,
		'alias-fragment': 1,
		open: 1,
		'owned-open': 1,
		all: 1,
		relationship: 1
	});
	const initial = engine.read((reader) => reader.record(TODO));
	assert.equal(initial.fields.title, 'original');
	assert.equal(initial.fields.description, null);
	assert.equal(Object.hasOwn(initial.fields, 'description'), true);
	assert.equal(Object.hasOwn(initial.fields, 'estimate'), false);
	assert.equal(initial.links.owner, USER);

	for (const key of notifications.keys()) notifications.set(key, 0);
	engine.batch((writer) => {
		// A partial fragment updates only fields it selected.
		writer.writeRecord({ key: TODO, revision: 2, fields: { title: 'updated' } });
	});

	assert.deepEqual(Object.fromEntries(notifications), {
		'by-pk': 1,
		'alias-fragment': 1,
		open: 1,
		'owned-open': 1,
		all: 1,
		relationship: 1
	});
	const updated = engine.read((reader) => reader.record(TODO));
	assert.equal(updated.fields.title, 'updated');
	assert.equal(updated.fields.description, null);
	assert.equal(Object.hasOwn(updated.fields, 'estimate'), false);
});

test('purpose-built engine keeps argument-sensitive roots distinct and batches watchers once', () => {
	const engine = createCacheEngine();
	const canonicalA = cacheIndexKey({
		field: 'todos',
		arguments: { where: { status: 'open', owner: 'one' }, first: 20 }
	});
	const canonicalB = cacheIndexKey({
		field: 'todos',
		arguments: { first: 20, where: { owner: 'one', status: 'open' } }
	});
	const closed = cacheIndexKey({
		field: 'todos',
		arguments: { first: 20, where: { owner: 'one', status: 'closed' } }
	});
	assert.equal(canonicalA, canonicalB);
	assert.notEqual(canonicalA, closed);

	let calls = 0;
	engine.watch((reader) => todoAt(reader, canonicalA), () => calls++);
	engine.batch((writer) => {
		writer.writeRecord({ key: TODO, revision: 1, fields: { id: 'todo-1', status: 'open' } });
		writer.writeIndex({ key: canonicalA, revision: 1, records: [TODO], complete: true });
		writer.writeRecord({ key: TODO, revision: 2, fields: { title: 'batched' } });
	});
	assert.equal(calls, 1);
	assert.equal(engine.read((reader) => reader.index(closed)), undefined);
});

test('named optimistic layers survive acceptance and stale base responses', () => {
	const engine = createCacheEngine();
	engine.batch((writer) => {
		writer.writeRecord({ key: TODO, revision: 4, fields: { status: 'open' } });
	});
	engine.createOptimisticLayer('complete-1', (writer) => {
		writer.writeRecord({ key: TODO, fields: { status: 'completed-optimistic' } });
	});
	assert.equal(engine.markOptimisticLayerAccepted('complete-1'), true);
	assert.equal(engine.optimisticLayerState('complete-1'), 'accepted');

	engine.batch((writer) => {
		assert.equal(
			writer.writeRecord({ key: TODO, revision: 3, fields: { status: 'open-stale' } }),
			false
		);
	});
	assert.equal(engine.read((reader) => todoFields(reader).status), 'completed-optimistic');
	assert.equal(engine.optimisticLayerState('complete-1'), 'accepted');
});

test('causal confirmation writes base and retires its layer atomically', () => {
	const engine = createCacheEngine();
	engine.batch((writer) => {
		writer.writeRecord({ key: TODO, revision: 1, fields: { status: 'open' } });
	});
	engine.createOptimisticLayer('complete-1', (writer) => {
		writer.writeRecord({ key: TODO, fields: { status: 'optimistic' } });
	});
	let calls = 0;
	const values = [];
	engine.watch(
		(reader) => reader.record(TODO)?.fields.status,
		(value) => {
			calls++;
			values.push(value);
		}
	);

	engine.confirmOptimisticLayer('complete-1', (writer) => {
		writer.writeRecord({ key: TODO, revision: 2, fields: { status: 'projected' } });
	});
	assert.equal(calls, 1);
	assert.deepEqual(values, ['projected']);
	assert.equal(engine.optimisticLayerState('complete-1'), undefined);
	assert.equal(engine.read((reader) => reader.record(TODO)?.fields.status), 'projected');
});

test('same-entity optimistic layers confirm and reject out of order without cross-rollback', () => {
	const engine = createCacheEngine();
	engine.batch((writer) => {
		writer.writeRecord({ key: TODO, revision: 1, fields: { title: 'base', status: 'open' } });
	});

	engine.createOptimisticLayer('title-a', (writer) => {
		writer.writeRecord({ key: TODO, fields: { title: 'A' } });
	});
	engine.createOptimisticLayer('title-b', (writer) => {
		writer.writeRecord({ key: TODO, fields: { title: 'B' } });
	});
	assert.equal(engine.read((reader) => reader.record(TODO)?.fields.title), 'B');

	// B confirms first. A stays tracked but cannot reappear above B's projection.
	engine.confirmOptimisticLayer('title-b', (writer) => {
		writer.writeRecord({ key: TODO, revision: 2, fields: { title: 'B projected' } });
	});
	assert.equal(engine.optimisticLayerState('title-a'), 'optimistic');
	assert.equal(engine.read((reader) => reader.record(TODO)?.fields.title), 'B projected');
	engine.confirmOptimisticLayer('title-a', (writer) => {
		assert.equal(
			writer.writeRecord({ key: TODO, revision: 1, fields: { title: 'A stale projection' } }),
			false
		);
	});
	assert.equal(engine.read((reader) => reader.record(TODO)?.fields.title), 'B projected');

	engine.createOptimisticLayer('status-c', (writer) => {
		writer.writeRecord({ key: TODO, fields: { status: 'C' } });
	});
	engine.createOptimisticLayer('status-d', (writer) => {
		writer.writeRecord({ key: TODO, fields: { status: 'D' } });
	});
	// C confirms first, but the causally later pending D remains visible.
	engine.confirmOptimisticLayer('status-c', (writer) => {
		writer.writeRecord({ key: TODO, revision: 3, fields: { status: 'C projected' } });
	});
	assert.equal(engine.read((reader) => reader.record(TODO)?.fields.status), 'D');
	engine.rejectOptimisticLayer('status-d');
	assert.equal(engine.read((reader) => reader.record(TODO)?.fields.status), 'C projected');

	engine.createOptimisticLayer('title-e', (writer) => {
		writer.writeRecord({ key: TODO, fields: { title: 'E' } });
	});
	engine.createOptimisticLayer('title-f', (writer) => {
		writer.writeRecord({ key: TODO, fields: { title: 'F' } });
	});
	engine.rejectOptimisticLayer('title-f');
	assert.equal(engine.read((reader) => reader.record(TODO)?.fields.title), 'E');
	engine.confirmOptimisticLayer('title-e', (writer) => {
		writer.writeRecord({ key: TODO, revision: 4, fields: { title: 'E projected' } });
	});
	assert.equal(engine.read((reader) => reader.record(TODO)?.fields.title), 'E projected');
});

test('record revisions and tombstones reject stale overwrite and resurrection', () => {
	const engine = createCacheEngine();
	engine.batch((writer) => {
		writer.writeRecord({ key: TODO, revision: '8', fields: { title: 'latest', status: 'open' } });
		assert.equal(writer.tombstoneRecord(TODO, 9n), true);
	});
	assert.equal(engine.read((reader) => reader.record(TODO)), undefined);

	engine.batch((writer) => {
		assert.equal(writer.writeRecord({ key: TODO, revision: 8, fields: { title: 'stale' } }), false);
		assert.equal(writer.writeRecord({ key: TODO, revision: 9, fields: { title: 'same' } }), false);
		assert.equal(writer.tombstoneRecord(TODO, 7), false);
	});
	assert.equal(engine.read((reader) => reader.record(TODO)), undefined);

	engine.batch((writer) => {
		assert.equal(
			writer.writeRecord({ key: TODO, revision: 10, fields: { id: 'todo-1', title: 'recreated' } }),
			true
		);
	});
	const recreated = engine.read((reader) => reader.record(TODO));
	assert.equal(recreated.fields.title, 'recreated');
	assert.equal(Object.hasOwn(recreated.fields, 'status'), false);
});

test('SSR extract and restore contain confirmed base only', () => {
	const engine = createCacheEngine();
	engine.batch((writer) => {
		writer.writeRecord({ key: TODO, revision: 1, fields: { title: 'confirmed' } });
		writer.writeIndex({ key: ALL, revision: 1, records: [TODO], complete: true });
	});
	engine.createOptimisticLayer('rename-1', (writer) => {
		writer.writeRecord({ key: TODO, fields: { title: 'optimistic secret' } });
		writer.writeRecord({ key: OTHER_TODO, fields: { title: 'optimistic insert' } });
		writer.writeIndex({ key: ALL, records: [TODO, OTHER_TODO], complete: true });
	});

	const snapshot = engine.extract();
	const serialized = JSON.stringify(snapshot);
	assert.doesNotMatch(serialized, /optimistic/);
	assert.equal(engine.read((reader) => reader.record(TODO)?.fields.title), 'optimistic secret');

	const restored = createCacheEngine();
	restored.restore(JSON.parse(serialized));
	assert.equal(restored.read((reader) => reader.record(TODO)?.fields.title), 'confirmed');
	assert.equal(restored.read((reader) => reader.record(OTHER_TODO)), undefined);
	assert.deepEqual(restored.read((reader) => reader.index(ALL)?.records), [TODO]);
	assert.equal(restored.optimisticLayerState('rename-1'), undefined);
});

test('GC traverses confirmed indexes and relationship links while retaining tombstone fences', () => {
	const engine = createCacheEngine();
	engine.batch((writer) => {
		writer.writeRecord({ key: USER, revision: 1, fields: { name: 'Ada' }, links: { todos: [TODO] } });
		writer.writeRecord({ key: TODO, revision: 1, fields: { title: 'reachable' } });
		writer.writeRecord({ key: OTHER_TODO, revision: 1, fields: { title: 'orphan' } });
		writer.writeRecord({ key: 'Todo:deleted', revision: 1, fields: { title: 'deleted' } });
		writer.tombstoneRecord('Todo:deleted', 2);
		writer.writeIndex({ key: ALL, revision: 1, records: [USER], complete: true });
	});
	assert.deepEqual(engine.gc(), [OTHER_TODO]);
	assert.equal(engine.read((reader) => reader.record(TODO)?.fields.title), 'reachable');

	engine.batch((writer) => writer.deleteIndex(ALL, 2));
	engine.retain(USER);
	assert.deepEqual(engine.gc(), []);
	engine.release(USER);
	assert.deepEqual(engine.gc(), [TODO, USER]);
	assert.match(JSON.stringify(engine.extract()), /Todo:deleted/);
});

test('GC cannot corrupt rollback beneath destructive optimistic overlays', () => {
	const engine = createCacheEngine();
	engine.batch((writer) => {
		writer.writeRecord({ key: USER, revision: 1, fields: { name: 'Ada' }, links: { todos: [TODO] } });
		writer.writeRecord({ key: TODO, revision: 1, fields: { title: 'confirmed child' } });
		writer.writeRecord({ key: OTHER_TODO, revision: 1, fields: { title: 'optimistic root' } });
		writer.writeIndex({ key: ALL, revision: 1, records: [USER], complete: true });
	});

	engine.createOptimisticLayer('rewrite-roots', (writer) => {
		writer.tombstoneRecord(USER);
		writer.writeIndex({ key: ALL, records: [OTHER_TODO], complete: true });
	});
	assert.deepEqual(engine.gc(), []);
	engine.rejectOptimisticLayer('rewrite-roots');
	assert.deepEqual(engine.read((reader) => reader.index(ALL)?.records), [USER]);
	assert.deepEqual(engine.read((reader) => reader.record(USER)?.links.todos), [TODO]);
	assert.equal(engine.read((reader) => reader.record(TODO)?.fields.title), 'confirmed child');
	assert.deepEqual(engine.gc(), [OTHER_TODO]);

	engine.createOptimisticLayer('delete-root', (writer) => writer.deleteIndex(ALL));
	assert.deepEqual(engine.gc(), []);
	engine.rejectOptimisticLayer('delete-root');
	assert.deepEqual(engine.read((reader) => reader.index(ALL)?.records), [USER]);
	assert.equal(engine.read((reader) => reader.record(TODO)?.fields.title), 'confirmed child');
});

test('GC preserves every record that can reappear from a non-current optimistic layer', () => {
	const engine = createCacheEngine();
	engine.batch((writer) => {
		writer.writeRecord({ key: USER, revision: 1, fields: { name: 'Ada' } });
		writer.writeRecord({ key: TODO, revision: 1, fields: { title: 'hidden by top layer' } });
		writer.writeIndex({ key: USER_TODOS, revision: 1, records: [USER], complete: true });
	});

	engine.createOptimisticLayer('older-root-and-link', (writer) => {
		writer.writeRecord({ key: USER, links: { todos: [TODO] } });
		writer.writeIndex({ key: ALL, records: [TODO], complete: true });
	});
	engine.createOptimisticLayer('newer-hides-root-and-link', (writer) => {
		writer.writeRecord({ key: USER, links: { todos: [] } });
		writer.writeIndex({ key: ALL, records: [], complete: true });
	});
	assert.deepEqual(engine.read((reader) => reader.record(USER)?.links.todos), []);
	assert.deepEqual(engine.read((reader) => reader.index(ALL)?.records), []);
	assert.deepEqual(engine.gc(), []);

	engine.rejectOptimisticLayer('newer-hides-root-and-link');
	assert.deepEqual(engine.read((reader) => reader.record(USER)?.links.todos), [TODO]);
	assert.deepEqual(engine.read((reader) => reader.index(ALL)?.records), [TODO]);
	assert.equal(engine.read((reader) => reader.record(TODO)?.fields.title), 'hidden by top layer');

	engine.rejectOptimisticLayer('older-root-and-link');
	assert.deepEqual(engine.gc(), [TODO]);
});

test('failed batches restore base state without notifying observers', () => {
	const engine = createCacheEngine();
	engine.batch((writer) => writer.writeRecord({ key: TODO, revision: 1, fields: { title: 'base' } }));
	let calls = 0;
	engine.watch((reader) => reader.record(TODO)?.fields.title, () => calls++);
	assert.throws(
		() =>
			engine.batch((writer) => {
				writer.writeRecord({ key: TODO, revision: 2, fields: { title: 'should roll back' } });
				throw new Error('abort');
			}),
		/abort/
	);
	assert.equal(engine.read((reader) => reader.record(TODO)?.fields.title), 'base');
	assert.equal(calls, 0);
});

const APOLLO_TODO = gql`
	fragment CacheEngineTodo on Todo {
		id
		status
	}
	query CacheEngineTodoQuery {
		todo {
			__typename
			...CacheEngineTodo
		}
	}
`;

function apolloWrite(cache, status) {
	cache.writeQuery({
		query: APOLLO_TODO,
		data: { todo: { __typename: 'Todo', id: 'todo-1', status } }
	});
}

test('Apollo 4 public APIs pass basic layers/batching/extract but expose the causal ordering gap', () => {
	const cache = new InMemoryCache({
		typePolicies: { Todo: { keyFields: ['id'] } }
	});
	apolloWrite(cache, 'base');
	let calls = 0;
	cache.watch({ query: APOLLO_TODO, optimistic: true, callback: () => calls++ });
	cache.batch({
		optimistic: 'A',
		update() {
			apolloWrite(cache, 'A');
		}
	});
	cache.batch({
		optimistic: 'B',
		update() {
			apolloWrite(cache, 'B');
		}
	});
	assert.equal(cache.readQuery({ query: APOLLO_TODO }, true).todo.status, 'B');
	assert.equal(calls, 2, 'each public batch broadcasts once');

	const baseSnapshot = JSON.stringify(cache.extract(false));
	const optimisticSnapshot = JSON.stringify(cache.extract(true));
	assert.match(baseSnapshot, /base/);
	assert.doesNotMatch(baseSnapshot, /"status":"A"|"status":"B"/);
	assert.match(optimisticSnapshot, /"status":"B"/);

	cache.batch({
		update() {
			apolloWrite(cache, 'B projected');
		},
		removeOptimistic: 'B'
	});
	// Native Apollo layer ordering reveals older A after newer B confirms.
	assert.equal(cache.readQuery({ query: APOLLO_TODO }, true).todo.status, 'A');
	cache.removeOptimistic('A');
	assert.equal(cache.readQuery({ query: APOLLO_TODO }, true).todo.status, 'B projected');

	// Apollo has no source-revision/tombstone fence: arrival order can resurrect stale data.
	cache.evict({ id: 'Todo:{"id":"todo-1"}' });
	apolloWrite(cache, 'stale resurrection');
	assert.equal(cache.readQuery({ query: APOLLO_TODO }).todo.status, 'stale resurrection');
});

test('built declarations contain no Apollo/vendor types', async () => {
	const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
	const declarationFiles = [];
	async function visit(directory) {
		for (const entry of await readdir(directory, { withFileTypes: true })) {
			const path = join(directory, entry.name);
			if (entry.isDirectory()) await visit(path);
			else if (entry.name.endsWith('.d.ts')) declarationFiles.push(path);
		}
	}
	await visit(join(packageRoot, 'dist'));
	assert.ok(declarationFiles.length > 0);
	for (const path of declarationFiles) {
		const declaration = await readFile(path, 'utf8');
		assert.doesNotMatch(declaration, /@apollo\/client|\bApollo(?:Cache|Client|Link)\b/);
	}
});
