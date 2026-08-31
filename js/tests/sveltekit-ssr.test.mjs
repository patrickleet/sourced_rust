import assert from 'node:assert/strict';
import test from 'node:test';

import {
	createDistributedSvelteKit,
	createDistributedSvelteKitServer,
	defineDistributedBoundaryBinding,
	defineDistributedBoundaryOperation
} from '../dist/sveltekit/index.js';
import {
	REACT_FIXTURE_SCHEMA,
	TodosArtifact,
	todoFrame
} from './fixtures/adapter-conformance.mjs';

function jsonResponse(body, status = 200) {
	return {
		status,
		statusText: status === 200 ? 'OK' : 'Error',
		text: async () => JSON.stringify(body)
	};
}

async function flushMicrotasks() {
	for (let iteration = 0; iteration < 16; iteration += 1) {
		await Promise.resolve();
	}
}

class SsrWebSocket {
	static CONNECTING = 0;
	static OPEN = 1;
	static CLOSING = 2;
	static CLOSED = 3;
	static instances = [];

	readyState = SsrWebSocket.CONNECTING;
	closed = false;
	sent = [];

	constructor(url, protocol) {
		this.url = url;
		this.protocol = protocol;
		SsrWebSocket.instances.push(this);
	}

	send(value) {
		this.sent.push(JSON.parse(value));
	}

	close() {
		this.closed = true;
		this.readyState = SsrWebSocket.CLOSED;
	}

	open() {
		this.readyState = SsrWebSocket.OPEN;
		this.onopen?.();
	}

	receive(message) {
		this.onmessage?.({ data: JSON.stringify(message) });
	}
}

const todosBoundary = defineDistributedBoundaryOperation(
	{
		operation: 'Todos',
		route: '/todos',
		kind: 'page',
		discovery: 'route_document'
	},
	TodosArtifact,
	defineDistributedBoundaryBinding(TodosArtifact, {})
);

function serverHarness() {
	const calls = [];
	const server = createDistributedSvelteKitServer({
		boundaries: [todosBoundary],
		getSession: async (event) => event.locals.session,
		getRole: () => 'user'
	});
	const event = (token, position = '1') => ({
		locals: {
			session: {
				accessToken: token,
				user: { id: token }
			}
		},
		route: { id: '/todos' },
		url: new URL('https://app.example/todos'),
		async fetch(url, init) {
			const body = JSON.parse(init.body);
			const authorization = init.headers.authorization;
			calls.push({ url, init, body, authorization });
			assert.deepEqual(body.extensions.distributed.client, {
				surface: { kind: 'role', name: 'user' },
				schemaHash: REACT_FIXTURE_SCHEMA
			});
			assert.equal(authorization, `Bearer ${token}`);
			return jsonResponse(
				todoFrame(
					TodosArtifact,
					[
						{
							id: `todo-${token}`,
							title: `${token}:${position}`,
							status: 'open'
						}
					],
					{
						cacheScope: `cache:${token}`,
						position
					}
				)
			);
		}
	});
	return {
		server,
		event,
		calls
	};
}

test('static @load SSR is request-isolated and hydration avoids a duplicate first fetch', async () => {
	const harness = serverHarness();
	const [alice, bob] = await Promise.all([
		harness.server.load(harness.event('alice')),
		harness.server.load(harness.event('bob'))
	]);

	assert.equal(harness.calls.length, 2);
	assert.equal(alice.gqlError, null);
	assert.equal(bob.gqlError, null);
	assert.equal(alice.distributed.state.scope.cacheScope, 'cache:alice');
	assert.equal(bob.distributed.state.scope.cacheScope, 'cache:bob');
	assert.equal(JSON.stringify(alice.distributed).includes('bob'), false);
	assert.equal(JSON.stringify(bob.distributed).includes('alice'), false);

	SsrWebSocket.instances.length = 0;
	let browserFetches = 0;
	const client = createDistributedSvelteKit({
		boundaries: [todosBoundary],
		session: { getAuth: () => ({ accessToken: 'alice' }) },
		hydration: alice.distributed,
		authority: alice.distributedAuthority,
		fetch: async () => {
			browserFetches += 1;
			throw new Error('hydrated first render must not fetch');
		},
		webSocket: SsrWebSocket
	});
	const todos = client.operation(TodosArtifact).use();
	assert.equal(todos.get().status, 'ready');
	assert.equal(todos.get().data.todos[0].title, 'alice:1');
	assert.equal(browserFetches, 0);
	assert.equal(SsrWebSocket.instances.length, 0);

	const unsubscribe = todos.subscribe(() => undefined);
	await flushMicrotasks();
	assert.equal(browserFetches, 0);
	assert.equal(SsrWebSocket.instances.length, 1, '@live attaches after hydration');
	const socket = SsrWebSocket.instances[0];
	socket.open();
	socket.receive({ type: 'connection_ack' });
	socket.receive({
		type: 'next',
		id: '1',
		payload: todoFrame(
			TodosArtifact,
			[{ id: 'todo-alice', title: 'alice:1', status: 'open' }],
			{ cacheScope: 'cache:alice', position: '1', source: 'live' }
		)
	});
	await flushMicrotasks();
	assert.equal(
		browserFetches,
		0,
		'query-to-live handoff must not fetch through transient cache state'
	);
	unsubscribe();
	assert.equal(socket.closed, true);
	client.destroy();
});

test('client-side data requests skip GraphQL so navigation stays SPA', async () => {
	const harness = serverHarness();
	const document = await harness.server.load(harness.event('alice'));
	assert.equal(harness.calls.length, 1);
	assert.ok(document.distributed);

	const dataNav = await harness.server.load({
		...harness.event('alice'),
		isDataRequest: true
	});
	assert.equal(harness.calls.length, 1, 'SPA data request must not seed a new replica');
	assert.equal(dataNav.distributed, undefined);
	assert.equal(dataNav.gqlError, null);
	assert.equal(dataNav.accessToken, 'alice');
});

test('replica state never self-authorizes hydration scope', async () => {
	const harness = serverHarness();
	const [alice, bob] = await Promise.all([
		harness.server.load(harness.event('alice')),
		harness.server.load(harness.event('bob'))
	]);

	assert.throws(
		() =>
			createDistributedSvelteKit({
				boundaries: [todosBoundary],
				session: { getAuth: () => ({ accessToken: 'alice' }) },
				hydration: alice.distributed
			}),
		/separate trusted SSR authority/
	);

	const client = createDistributedSvelteKit({
		boundaries: [todosBoundary],
		session: { getAuth: () => ({ accessToken: 'bob' }) }
	});
	assert.equal(
		client.hydrate(alice.distributed, bob.distributedAuthority),
		false,
		'a replayed user-A state fails against current user-B page authority'
	);
	assert.equal(client.replica.scope, undefined);

	const tampered = structuredClone(alice.distributed);
	tampered.state.scope.cacheScope = 'cache:forged';
	assert.equal(
		client.hydrate(tampered, alice.distributedAuthority),
		false,
		'tampered state scope fails against the separately carried authority'
	);
	assert.equal(client.replica.scope, undefined);
	client.destroy();
});

test('failed reload registration disposes partially constructed client resources', () => {
	let unsubscribed = false;
	let runtimeDisposed = false;
	assert.throws(
		() =>
			createDistributedSvelteKit({
				session: {
					getAuth: () => ({}),
					subscribe: () => () => {
						unsubscribed = true;
					}
				},
				browser: true,
				reload: { key: '' },
				createCommands: () => ({
					commands: Object.freeze({}),
					dispose() {
						runtimeDisposed = true;
					}
				})
			}),
		/reload participant key/
	);
	assert.equal(runtimeDisposed, true);
	assert.equal(unsubscribed, true);
});

test('route misses do no GraphQL work and same-scope soft-nav merges overlapping seed', async () => {
	const harness = serverHarness();
	const missed = await harness.server.load({
		...harness.event('alice'),
		route: { id: '/session' },
		url: new URL('https://app.example/session')
	});
	assert.equal(harness.calls.length, 0);
	assert.equal(missed.distributed, undefined);
	assert.equal(missed.gqlError, null);

	const first = await harness.server.load(harness.event('alice', '1'));
	const second = await harness.server.load(harness.event('alice', '2'));
	let browserFetches = 0;
	const client = createDistributedSvelteKit({
		boundaries: [todosBoundary],
		session: { getAuth: () => ({ accessToken: 'alice' }) },
		hydration: first.distributed,
		authority: first.distributedAuthority,
		fetch: async () => {
			browserFetches += 1;
			throw new Error('navigation hydration must not fetch');
		},
		webSocket: SsrWebSocket
	});
	const todos = client.operation(TodosArtifact).use({}, { live: false });
	const values = [];
	const unsubscribe = todos.subscribe((snapshot) => {
		values.push(snapshot.data.todos?.[0]?.title);
	});
	assert.equal(todos.get().data.todos[0].title, 'alice:1');
	// Overlapping same-scope seed upserts the route window (position/title update)
	// without requiring a browser GraphQL round-trip.
	assert.equal(
		client.hydrate(second.distributed, second.distributedAuthority),
		true
	);
	assert.equal(todos.get().data.todos[0].title, 'alice:2');
	assert.equal(values.at(-1), 'alice:2');
	assert.equal(browserFetches, 0);
	unsubscribe();
	client.destroy();
});

test('auth changes purge old data, abort live work, and reject cross-scope hydration', async () => {
	const harness = serverHarness();
	const [alice, bob] = await Promise.all([
		harness.server.load(harness.event('alice')),
		harness.server.load(harness.event('bob'))
	]);
	let credential = { accessToken: 'alice' };
	const listeners = new Set();
	const requests = [];
	SsrWebSocket.instances.length = 0;
	const client = createDistributedSvelteKit({
		boundaries: [todosBoundary],
		session: {
			getAuth: () => credential,
			subscribe(listener) {
				listeners.add(listener);
				return () => listeners.delete(listener);
			}
		},
		hydration: alice.distributed,
		authority: alice.distributedAuthority,
		fetch: (url, init) => {
			requests.push({ url, init });
			return new Promise(() => undefined);
		},
		webSocket: SsrWebSocket
	});
	const todos = client.operation(TodosArtifact).use();
	const unsubscribe = todos.subscribe(() => undefined);
	await flushMicrotasks();
	assert.equal(todos.get().data.todos[0].title, 'alice:1');
	assert.equal(SsrWebSocket.instances.length, 1);

	assert.equal(
		client.hydrate(bob.distributed, alice.distributedAuthority),
		false,
		'replayed state cannot authorize itself against current page authority'
	);
	assert.equal(todos.get().complete, false);
	assert.equal(SsrWebSocket.instances[0].closed, true);

	credential = {};
	for (const listener of listeners) listener();
	await flushMicrotasks();
	assert.equal(todos.get().complete, false);
	assert.deepEqual(todos.get().data, {});
	assert.equal(requests.length, 2);
	assert.equal(requests[0].init.signal.aborted, true);
	assert.equal(requests[1].init.headers.authorization, undefined);

	unsubscribe();
	client.destroy();
});

test('one browser replica refuses to mix user and elevated generated surfaces', async () => {
	const harness = serverHarness();
	const alice = await harness.server.load(harness.event('alice'));
	const client = createDistributedSvelteKit({
		boundaries: [todosBoundary],
		session: { getAuth: () => ({ accessToken: 'alice' }) },
		hydration: alice.distributed,
		authority: alice.distributedAuthority
	});
	client.operation(TodosArtifact).read({});

	const adminOperation = Object.freeze({
		...TodosArtifact,
		id: 'query:admin-todos',
		document: 'query AdminTodos { todos { id title status } }',
		protocol: Object.freeze({
			...TodosArtifact.protocol,
			surface: Object.freeze({ kind: 'role', name: 'admin' }),
			operation: 'query:admin-todos'
		}),
		live: undefined
	});
	assert.throws(
		() => client.operation(adminOperation).read({}),
		/active replica binding/
	);
	client.destroy();
});
