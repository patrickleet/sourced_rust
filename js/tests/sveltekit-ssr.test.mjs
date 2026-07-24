import assert from 'node:assert/strict';
import test from 'node:test';

import {
	createDistributedSvelteKit,
	createDistributedSvelteKitServer
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
}

function routeBinding(artifact = TodosArtifact) {
	return Object.freeze({
		plan: Object.freeze({
			operation: 'Todos',
			route: '/todos',
			discovery: 'convention'
		}),
		artifact
	});
}

function serverHarness() {
	const calls = [];
	let variablesCalls = 0;
	const server = createDistributedSvelteKitServer({
		routes: [routeBinding()],
		getSession: async (event) => event.locals.session,
		getRole: () => 'user',
		variables: {
			Todos: () => {
				variablesCalls += 1;
				return {};
			}
		}
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
		calls,
		get variablesCalls() {
			return variablesCalls;
		}
	};
}

test('static @load SSR is request-isolated and hydration avoids a duplicate first fetch', async () => {
	const harness = serverHarness();
	const [alice, bob] = await Promise.all([
		harness.server.load(harness.event('alice')),
		harness.server.load(harness.event('bob'))
	]);

	assert.equal(harness.calls.length, 2);
	assert.equal(harness.variablesCalls, 2, 'variables run exactly once per request');
	assert.equal(alice.gqlError, null);
	assert.equal(bob.gqlError, null);
	assert.equal(alice.distributed.state.scope.cacheScope, 'cache:alice');
	assert.equal(bob.distributed.state.scope.cacheScope, 'cache:bob');
	assert.equal(JSON.stringify(alice.distributed).includes('bob'), false);
	assert.equal(JSON.stringify(bob.distributed).includes('alice'), false);

	SsrWebSocket.instances.length = 0;
	let browserFetches = 0;
	const client = createDistributedSvelteKit({
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
	unsubscribe();
	assert.equal(SsrWebSocket.instances[0].closed, true);
	client.destroy();
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
				session: { getAuth: () => ({ accessToken: 'alice' }) },
				hydration: alice.distributed
			}),
		/separate trusted SSR authority/
	);

	const client = createDistributedSvelteKit({
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

test('route misses do no GraphQL work and same-scope navigation replaces hydration', async () => {
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
