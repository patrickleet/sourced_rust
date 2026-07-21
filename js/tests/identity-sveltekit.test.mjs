import assert from 'node:assert/strict';
import { test } from 'node:test';

import { authIdentityKey, jwtPayloadSub } from '@hops-ops/distributed';
import { QueryCache, cacheKey } from '@hops-ops/distributed/cache';
import {
	authFromPageData,
	createUseGraphql
} from '@hops-ops/distributed/sveltekit';

function jwtLike(sub, extra = {}) {
	const encode = (value) => Buffer.from(JSON.stringify(value)).toString('base64url');
	return `${encode({ alg: 'RS256', typ: 'JWT' })}.${encode({ sub, ...extra })}.signature`;
}

function deferred() {
	let resolve;
	const promise = new Promise((done) => {
		resolve = done;
	});
	return { promise, resolve };
}

async function waitFor(predicate, message) {
	for (let attempt = 0; attempt < 100; attempt += 1) {
		if (predicate()) return;
		await new Promise((resolve) => setImmediate(resolve));
	}
	assert.fail(message);
}

test('identity keys use JWT subject, opaque token hash, or DevHeaders principal', () => {
	const aliceOld = jwtLike('alice', { generation: 1 });
	const aliceNew = jwtLike('alice', { generation: 2 });
	const bob = jwtLike('bob');

	assert.equal(jwtPayloadSub(aliceOld), 'alice');
	assert.equal(authIdentityKey({ accessToken: aliceOld }), 'sub:alice');
	assert.equal(authIdentityKey({ accessToken: aliceNew }), 'sub:alice');
	assert.equal(authIdentityKey({ accessToken: bob }), 'sub:bob');
	assert.equal(jwtPayloadSub('opaque'), null);
	assert.match(authIdentityKey({ accessToken: 'opaque' }), /^bearer:[0-9a-f]+$/);
	assert.notEqual(
		authIdentityKey({ accessToken: 'opaque-a' }),
		authIdentityKey({ accessToken: 'opaque-b' })
	);
	assert.equal(authIdentityKey({ userId: 'alice', role: 'admin' }), 'dev:alice:admin');
	assert.notEqual(
		authIdentityKey({ userId: 'alice', role: 'admin' }),
		authIdentityKey({ userId: 'alice', role: 'user' })
	);
});

test('page auth prefers access token and falls back to session user plus role', () => {
	assert.deepEqual(
		authFromPageData({
			accessToken: 'top-level',
			session: { accessToken: 'session', user: { id: 'alice' } },
			engineRole: 'admin'
		}),
		{ accessToken: 'top-level', userId: undefined, role: 'admin' }
	);
	assert.deepEqual(
		authFromPageData({
			accessToken: null,
			session: { accessToken: null, user: { id: 'alice' } },
			engineRole: 'writer'
		}),
		{ accessToken: null, userId: 'alice', role: 'writer' }
	);
});

test('createUseGraphql clears the original identity before the first switched request', async () => {
	const alice = jwtLike('alice');
	const bob = jwtLike('bob');
	let page = { accessToken: alice, session: null, engineRole: 'user' };
	const cache = new QueryCache();
	const privateDocument = 'query PrivateTodos { todos { id secret } }';
	const privateKey = cacheKey(privateDocument);
	const fetchCalls = [];

	const useGraphql = createUseGraphql({ url: () => '/graphql' });
	const gql = useGraphql(() => page, {
		cache,
		client: {
			fetch: async (url, init) => {
				fetchCalls.push({ url, init });
				return {
					status: 200,
					json: async () => ({ data: { viewer: { id: 'bob' } } })
				};
			}
		}
	});

	cache.set(privateKey, {
		data: { todos: [{ id: 'a', secret: 'alice-only' }] },
		updatedAt: 1
	});
	page = { accessToken: bob, session: null, engineRole: 'user' };

	const result = await gql.request('query Viewer { viewer { id } }');
	assert.deepEqual(result.data, { viewer: { id: 'bob' } });
	assert.equal(cache.get(privateKey), undefined);
	assert.equal(fetchCalls[0].url, '/graphql');
	assert.equal(fetchCalls[0].init.headers.authorization, `Bearer ${bob}`);

	const bobKey = cacheKey('query BobOnly { viewer { id } }');
	cache.set(bobKey, { data: { viewer: { id: 'bob' } }, updatedAt: 2 });
	await gql.request('query ViewerAgain { viewer { id } }');
	assert.deepEqual(cache.get(bobKey)?.data, { viewer: { id: 'bob' } });
});

test('createUseGraphql captures async custom auth at bind time', async () => {
	let identity = 'alice';
	let releaseInitial;
	const initialGate = new Promise((resolve) => {
		releaseInitial = resolve;
	});
	let authCalls = 0;
	const cache = new QueryCache();
	const key = cacheKey('query Secret { secret }');
	const useGraphql = createUseGraphql({
		getAuth: async (data) => {
			authCalls += 1;
			if (authCalls === 1) await initialGate;
			return { userId: data.session?.user?.id, role: data.engineRole };
		}
	});
	const gql = useGraphql(
		() => ({ session: { user: { id: identity } }, engineRole: 'user' }),
		{
			cache,
			client: {
				fetch: async () => ({
					status: 200,
					json: async () => ({ data: { ok: true } })
				})
			}
		}
	);
	cache.set(key, { data: { secret: 'alice' }, updatedAt: 1 });
	identity = 'bob';
	releaseInitial();
	await gql.request('query Ok { ok }');
	assert.equal(cache.get(key), undefined);
});

test('an auth mapping failure clears private cache before rejecting', async () => {
	let unavailable = false;
	const document = 'query Secret { secret }';
	const key = cacheKey(document);
	const cache = new QueryCache();
	const gql = createUseGraphql({
		getAuth: async () => {
			if (unavailable) throw new Error('auth unavailable');
			return { userId: 'alice', role: 'user' };
		}
	})(() => ({ session: { user: { id: 'alice' } }, engineRole: 'user' }), {
		cache,
		client: {
			fetch: async () => ({
				status: 200,
				json: async () => ({ data: { secret: 'alice-only' } })
			})
		}
	});

	await gql.request(document);
	assert.deepEqual(cache.get(key)?.data, { secret: 'alice-only' });
	const generation = cache.generation;
	unavailable = true;
	await assert.rejects(() => gql.request(document), /auth unavailable/);
	assert.equal(cache.get(key), undefined);
	assert.ok(cache.generation > generation);
});

test('a late response from the previous principal cannot overwrite the current cache', async () => {
	const alice = jwtLike('alice');
	const bob = jwtLike('bob');
	let page = { accessToken: alice, session: null, engineRole: 'user' };
	const cache = new QueryCache();
	const document = 'query Viewer { viewer { id } }';
	const key = cacheKey(document);
	const aliceStarted = deferred();
	const bobStarted = deferred();
	const aliceResponse = deferred();
	const bobResponse = deferred();

	const gql = createUseGraphql()(() => page, {
		cache,
		client: {
			fetch: async (_url, init) => {
				if (init.headers.authorization === `Bearer ${alice}`) {
					aliceStarted.resolve();
					return aliceResponse.promise;
				}
				assert.equal(init.headers.authorization, `Bearer ${bob}`);
				bobStarted.resolve();
				return bobResponse.promise;
			}
		}
	});

	const oldRequest = gql.request(document);
	await aliceStarted.promise;
	page = { accessToken: bob, session: null, engineRole: 'user' };
	const currentRequest = gql.request(document);
	await bobStarted.promise;

	bobResponse.resolve({
		status: 200,
		json: async () => ({ data: { viewer: { id: 'bob' } } })
	});
	await currentRequest;
	assert.deepEqual(cache.get(key)?.data, { viewer: { id: 'bob' } });

	aliceResponse.resolve({
		status: 200,
		json: async () => ({ data: { viewer: { id: 'alice' } } })
	});
	await oldRequest;
	assert.deepEqual(cache.get(key)?.data, { viewer: { id: 'bob' } });
});

class IdentityWebSocket {
	static OPEN = 1;
	static instances = [];

	constructor() {
		this.readyState = 0;
		this.closeCount = 0;
		this.sent = [];
		IdentityWebSocket.instances.push(this);
	}

	send(value) {
		this.sent.push(JSON.parse(value));
	}

	close() {
		this.readyState = 3;
		this.closeCount += 1;
	}

	open() {
		this.readyState = IdentityWebSocket.OPEN;
		this.onopen?.({});
	}

	message(message) {
		this.onmessage?.({ data: JSON.stringify(message) });
	}
}

test('an existing subscription closes before delivering data to a new principal', async () => {
	IdentityWebSocket.instances.length = 0;
	const alice = jwtLike('alice');
	const bob = jwtLike('bob');
	let page = { accessToken: alice, session: null, engineRole: 'user' };
	const cache = new QueryCache();
	const document = 'subscription PrivateEvents { private_events { id owner } }';
	const key = cacheKey(document);
	cache.set(key, { data: { private_events: [{ id: 'old', owner: 'alice' }] }, updatedAt: 1 });
	const delivered = [];
	const errors = [];
	const gql = createUseGraphql()(() => page, {
		cache,
		client: { webSocket: IdentityWebSocket }
	});
	const stop = gql.subscribe(document, {
		onNext: (payload) => delivered.push(payload),
		onError: (error) => errors.push(error)
	});

	await waitFor(
		() => IdentityWebSocket.instances.length === 1,
		'subscription socket was not created'
	);
	const socket = IdentityWebSocket.instances[0];
	socket.open();
	socket.message({ type: 'connection_ack' });
	page = { accessToken: bob, session: null, engineRole: 'user' };
	socket.message({
		type: 'next',
		id: '1',
		payload: { data: { private_events: [{ id: 'late', owner: 'alice' }] } }
	});

	await waitFor(() => socket.closeCount === 1, 'stale subscription was not closed');
	assert.deepEqual(delivered, []);
	assert.deepEqual(errors, []);
	assert.equal(cache.get(key), undefined);
	stop();
});
