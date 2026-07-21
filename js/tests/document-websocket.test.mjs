import assert from 'node:assert/strict';
import { test } from 'node:test';

import {
	createGraphqlClient,
	createDocumentStore,
	httpUrlToWsUrl,
	subscribe
} from '@hops-ops/distributed';
import { QueryCache, cacheKey } from '@hops-ops/distributed/cache';

const DOCUMENT = 'query Items($scope: String!) { items(scope: $scope) { id value } }';
const VARIABLES = { scope: 'mine' };

test('document store seeds, selects, follows cache notifications, and refetches', async () => {
	const cache = new QueryCache();
	let requestCount = 0;
	const client = {
		cache,
		async request(document, variables) {
			requestCount += 1;
			assert.equal(document, DOCUMENT);
			assert.deepEqual(variables, VARIABLES);
			return {
				data: { items: [{ id: 'server', value: 2 }] },
				status: 200
			};
		},
		subscribe() {
			return () => {};
		}
	};
	const store = createDocumentStore(client, {
		document: DOCUMENT,
		variables: VARIABLES,
		initialData: { items: [{ id: 'initial', value: 1 }] },
		select: (data) => data.items,
		list: { at: 'items', by: 'id' }
	});

	const snapshots = [];
	const unsubscribe = store.subscribe((snapshot) => snapshots.push(snapshot));
	assert.deepEqual(store.get().data, [{ id: 'initial', value: 1 }]);
	assert.deepEqual(store.target('items', 'id'), {
		document: DOCUMENT,
		variables: VARIABLES,
		at: 'items',
		by: 'id'
	});

	cache.set(cacheKey(DOCUMENT, VARIABLES), {
		data: { items: [{ id: 'cache', value: 3 }] },
		updatedAt: 3
	});
	assert.deepEqual(snapshots.at(-1).data, [{ id: 'cache', value: 3 }]);

	await store.refetch();
	assert.equal(requestCount, 1);
	assert.deepEqual(store.get().data, [{ id: 'server', value: 2 }]);
	assert.equal(store.get().error, null);

	unsubscribe();
	store.destroy();
});

test('document store reports refetch/live errors and cancels catch-up on destroy', async () => {
	const cache = new QueryCache();
	let requestCount = 0;
	let liveHandlers;
	let liveUnsubscribed = 0;
	const client = {
		cache,
		async request() {
			requestCount += 1;
			return { errors: [{ message: 'read denied' }], status: 403 };
		},
		subscribe(_document, handlers) {
			liveHandlers = handlers;
			return () => {
				liveUnsubscribed += 1;
			};
		}
	};
	const store = createDocumentStore(client, {
		document: DOCUMENT,
		variables: VARIABLES,
		live: true,
		select: (data) => data?.items ?? []
	});

	store.connect();
	assert.equal(store.get().status, 'connecting');
	liveHandlers.onNext({ data: { items: [{ id: 'live', value: 4 }] } });
	assert.equal(store.get().status, 'live');
	assert.deepEqual(store.get().data, [{ id: 'live', value: 4 }]);
	liveHandlers.onError('socket unavailable');
	assert.equal(store.get().status, 'error');
	assert.equal(store.get().error, 'socket unavailable');

	await store.refetch();
	assert.equal(store.get().error, 'read denied');
	store.scheduleCatchUp(10);
	store.destroy();
	await new Promise((resolve) => setTimeout(resolve, 30));
	assert.equal(requestCount, 1, 'destroy must cancel the scheduled second request');
	assert.equal(liveUnsubscribed, 1);
});

test('document store rejects clients without a cache', () => {
	assert.throws(
		() =>
			createDocumentStore(
				{
					async request() {
						return { status: 200 };
					},
					subscribe() {
						return () => {};
					}
				},
				{ document: DOCUMENT }
			),
		/requires a GraphqlClient with a cache/
	);
});

test('package-client refetch clears pending state when the projection catches up', async () => {
	const cache = new QueryCache();
	const key = cacheKey(DOCUMENT, VARIABLES);
	const projected = { items: [{ id: 'pending', value: 2 }] };
	cache.set(key, {
		data: projected,
		updatedAt: 1,
		pending: true,
		optimistic: true
	});
	const client = createGraphqlClient({
		getUrl: () => '/graphql',
		getAuth: () => ({}),
		cache,
		fetch: async () => ({
			status: 200,
			json: async () => ({ data: projected })
		})
	});
	const store = createDocumentStore(client, {
		document: DOCUMENT,
		variables: VARIABLES,
		list: { at: 'items', by: 'id' }
	});

	await store.refetch();
	assert.deepEqual(cache.get(key), {
		data: projected,
		updatedAt: cache.get(key).updatedAt,
		pending: false,
		optimistic: false
	});
	store.destroy();
});

class FakeWebSocket {
	static OPEN = 1;
	static instances = [];

	constructor(url, protocol) {
		this.url = url;
		this.protocol = protocol;
		this.readyState = 0;
		this.sent = [];
		this.closeCount = 0;
		FakeWebSocket.instances.push(this);
	}

	send(value) {
		this.sent.push(JSON.parse(value));
	}

	close() {
		this.closeCount += 1;
		this.readyState = 3;
	}

	open() {
		this.readyState = FakeWebSocket.OPEN;
		this.onopen?.({});
	}

	message(message) {
		this.onmessage?.({ data: JSON.stringify(message) });
	}
}

test('WebSocket transport performs graphql-transport-ws handshake and clean unsubscribe', () => {
	FakeWebSocket.instances.length = 0;
	const next = [];
	const errors = [];
	let completed = 0;
	const stop = subscribe(
		'subscription Events($room: ID!) { events(room: $room) { id } }',
		{ userId: 'alice', role: 'admin' },
		{
			onNext: (payload) => next.push(payload),
			onError: (error) => errors.push(error),
			onComplete: () => {
				completed += 1;
			}
		},
		{
			httpUrl: 'https://api.example/graphql?discard=this',
			variables: { room: 'lobby' },
			webSocket: FakeWebSocket
		}
	);

	const socket = FakeWebSocket.instances[0];
	assert.equal(socket.protocol, 'graphql-transport-ws');
	const url = new URL(socket.url);
	assert.equal(url.protocol, 'wss:');
	assert.equal(url.pathname, '/graphql/ws');
	assert.equal(url.searchParams.get('discard'), null);
	assert.equal(url.searchParams.get('x-user-id'), 'alice');
	assert.equal(url.searchParams.get('x-role'), 'admin');

	socket.open();
	assert.deepEqual(socket.sent[0], {
		type: 'connection_init',
		payload: { 'x-user-id': 'alice', 'x-role': 'admin' }
	});
	socket.message({ type: 'connection_ack' });
	assert.deepEqual(socket.sent[1], {
		type: 'subscribe',
		id: '1',
		payload: {
			query: 'subscription Events($room: ID!) { events(room: $room) { id } }',
			variables: { room: 'lobby' }
		}
	});

	socket.message({ type: 'next', id: '1', payload: { data: { events: [{ id: 'e1' }] } } });
	assert.deepEqual(next, [{ data: { events: [{ id: 'e1' }] } }]);
	socket.message({ type: 'ping' });
	assert.deepEqual(socket.sent[2], { type: 'pong' });
	socket.message({ type: 'error', id: '1', payload: [{ message: 'denied' }] });
	assert.deepEqual(errors, [[{ message: 'denied' }]]);
	socket.message({ type: 'complete', id: '1' });
	assert.equal(completed, 1);

	stop();
	stop();
	assert.deepEqual(socket.sent[3], { type: 'complete', id: '1' });
	assert.equal(socket.closeCount, 1);
});

test('HTTP URLs map to one WebSocket suffix and preserve secure transport', () => {
	assert.equal(
		httpUrlToWsUrl('http://127.0.0.1:8791/graphql/'),
		'ws://127.0.0.1:8791/graphql/ws'
	);
	assert.equal(
		httpUrlToWsUrl('https://api.example/graphql/ws'),
		'wss://api.example/graphql/ws'
	);
});
