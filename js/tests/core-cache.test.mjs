import assert from 'node:assert/strict';
import { test } from 'node:test';

import { parse } from 'graphql';
import {
	applyWsDevHeaderParams,
	buildAuthHeaders,
	createGraphqlClient,
	documentToString,
	looksLikeMutation,
	requestGraphql,
	wsConnectionInitPayload
} from '@hops-ops/distributed';
import {
	QueryCache,
	applyCacheOps,
	cacheKey,
	rollback,
	runCommandPipeline,
	writeServerDataPreservingPending
} from '@hops-ops/distributed/cache';

const TODOS = 'query Todos { todos { id title status } }';

test('HTTP and WebSocket auth prefer a trimmed bearer token over DevHeaders', () => {
	const auth = { accessToken: '  token  ', userId: 'ignored', role: 'admin' };
	assert.deepEqual(buildAuthHeaders(auth), {
		'content-type': 'application/json',
		authorization: 'Bearer token'
	});
	assert.deepEqual(wsConnectionInitPayload(auth), {
		authorization: 'Bearer token',
		accessToken: 'token'
	});

	const bearerUrl = new URL('ws://api.example/graphql/ws');
	applyWsDevHeaderParams(bearerUrl, auth);
	assert.equal(bearerUrl.search, '');

	assert.deepEqual(buildAuthHeaders({ userId: 'alice' }), {
		'content-type': 'application/json',
		'x-user-id': 'alice',
		'x-role': 'user'
	});
	assert.deepEqual(wsConnectionInitPayload({ userId: 'alice', role: 'writer' }), {
		'x-user-id': 'alice',
		'x-role': 'writer'
	});

	const devUrl = new URL('ws://api.example/graphql/ws?trace=1');
	applyWsDevHeaderParams(devUrl, { userId: 'alice', role: 'writer' });
	assert.equal(devUrl.searchParams.get('trace'), '1');
	assert.equal(devUrl.searchParams.get('x-user-id'), 'alice');
	assert.equal(devUrl.searchParams.get('x-role'), 'writer');
});

test('requestGraphql sends a normalized document, variables, and response status', async () => {
	let call;
	const document = parse('query Ping($value: Int!) { ping(value: $value) }');
	const result = await requestGraphql(
		'https://api.example/graphql',
		document,
		{ accessToken: 'abc' },
		{ value: 7 },
		{
			fetch: async (url, init) => {
				call = { url, init };
				return {
					status: 207,
					json: async () => ({ data: { ping: 'pong' } })
				};
			}
		}
	);

	assert.deepEqual(result, { data: { ping: 'pong' }, errors: undefined, status: 207 });
	assert.equal(call.url, 'https://api.example/graphql');
	assert.equal(call.init.method, 'POST');
	assert.equal(call.init.headers.authorization, 'Bearer abc');
	assert.deepEqual(JSON.parse(call.init.body), {
		query: documentToString(document),
		variables: { value: 7 }
	});
});

test('requestGraphql gives useful 401 errors for server and missing-token responses', async () => {
	const rejected = await requestGraphql('/graphql', 'query Q { q }', {}, {}, {
		fetch: async () => ({
			status: 401,
			json: async () => ({ errors: [{ message: 'token expired' }] })
		})
	});
	assert.deepEqual(rejected.errors, [{ message: 'token expired' }]);

	const missing = await requestGraphql('/graphql', 'query Q { q }', {}, {}, {
		fetch: async () => ({
			status: 401,
			json: async () => {
				throw new SyntaxError('not JSON');
			}
		})
	});
	assert.equal(missing.status, 401);
	assert.match(missing.errors?.[0]?.message ?? '', /access token/i);
});

test('requestGraphql reports non-GraphQL HTTP failures as transport errors', async () => {
	const plainText = await requestGraphql('/graphql', 'mutation Q { q }', {}, {}, {
		fetch: async () => ({
			status: 502,
			statusText: 'Bad Gateway',
			text: async () => 'upstream unavailable'
		})
	});
	assert.deepEqual(plainText, {
		data: undefined,
		errors: [{ message: 'HTTP 502 Bad Gateway: upstream unavailable' }],
		status: 502
	});

	const jsonError = await requestGraphql('/graphql', 'mutation Q { q }', {}, {}, {
		fetch: async () => ({
			status: 429,
			statusText: 'Too Many Requests',
			text: async () => JSON.stringify({ error: 'rate limited' })
		})
	});
	assert.deepEqual(jsonError.errors, [{ message: 'rate limited' }]);

	const graphqlError = await requestGraphql('/graphql', 'mutation Q { q }', {}, {}, {
		fetch: async () => ({
			status: 400,
			statusText: 'Bad Request',
			text: async () => JSON.stringify({ errors: [{ message: 'invalid mutation' }] })
		})
	});
	assert.deepEqual(graphqlError.errors, [{ message: 'invalid mutation' }]);

	const html = await requestGraphql('/graphql', 'mutation Q { q }', {}, {}, {
		fetch: async () => ({
			status: 503,
			statusText: 'Service Unavailable',
			text: async () => '<!doctype html><html><body>proxy failure</body></html>'
		})
	});
	assert.deepEqual(html.errors, [{ message: 'HTTP 503 Service Unavailable' }]);
});

test('an HTML command failure rolls back its optimistic row', async () => {
	const cache = new QueryCache();
	const key = cacheKey(TODOS);
	cache.set(key, {
		data: { todos: [{ id: 'existing', title: 'safe', status: 'open' }] },
		updatedAt: 1
	});

	const result = await runCommandPipeline(
		{
			cache,
			request: (document, variables) =>
				requestGraphql('/graphql', document, {}, variables ?? {}, {
					fetch: async () => ({
						status: 500,
						statusText: 'Internal Server Error',
						text: async () => '<html><body>proxy failure</body></html>'
					})
				})
		},
		'mutation Create($input: TodoInput!) { todos_create(input: $input) { id } }',
		{ id: 'ghost', title: 'temporary', status: 'open' },
		{
			browser: true,
			optimistic: {
				targets: [{ document: TODOS, at: 'todos', by: 'id' }],
				row: { id: 'ghost', title: 'temporary', status: 'open' }
			}
		}
	);

	assert.equal(result.status, 500);
	assert.deepEqual(result.errors, [{ message: 'HTTP 500 Internal Server Error' }]);
	assert.deepEqual(cache.get(key), {
		data: { todos: [{ id: 'existing', title: 'safe', status: 'open' }] },
		updatedAt: 1
	});
});

test('mutation detection parses leading comments, whitespace, and AST documents', () => {
	assert.equal(looksLikeMutation('# generated\n\n mutation Change { change }'), true);
	assert.equal(
		looksLikeMutation('fragment Fields on Query { ping }\nmutation Change { change }'),
		true
	);
	assert.equal(looksLikeMutation(parse('mutation Change { change }')), true);
	assert.equal(looksLikeMutation('query Read { value }'), false);
	assert.equal(looksLikeMutation('not valid GraphQL'), false);
});

test('GraphqlClient writes successful queries but never mutation-shaped results', async () => {
	const cache = new QueryCache();
	const calls = [];
	const client = createGraphqlClient({
		getUrl: () => '/graphql',
		getAuth: async () => ({ userId: 'dev-user', role: 'user' }),
		cache,
		fetch: async (_url, init) => {
			const body = JSON.parse(init.body);
			calls.push(body);
			return body.query.includes('mutation')
				? {
						status: 202,
						json: async () => ({ data: { todos_create: { id: 'new' } } })
					}
				: {
						status: 200,
						json: async () => ({ data: { todos: [{ id: 'one' }] } })
					};
		}
	});

	const variables = { filter: { status: 'open' }, page: 1 };
	await client.request(TODOS, variables);
	assert.deepEqual(cache.get(cacheKey(TODOS, variables))?.data, {
		todos: [{ id: 'one' }]
	});

	const mutation = '# generated operation\nmutation Create { todos_create { id } }';
	await client.request(mutation);
	assert.equal(cache.get(cacheKey(mutation)), undefined);
	assert.equal(calls.length, 2);
});

test('QueryCache keys are stable, exact invalidation is isolated, and cycles fail clearly', () => {
	assert.equal(
		cacheKey(TODOS, { nested: { z: 1, a: 2 }, page: 3 }),
		cacheKey(TODOS, { page: 3, nested: { a: 2, z: 1 } })
	);
	assert.equal(cacheKey(TODOS), cacheKey(TODOS, {}));

	const cache = new QueryCache();
	cache.set('doc::', { data: 1, updatedAt: 1 });
	cache.set('doc::sibling', { data: 2, updatedAt: 1 });
	cache.invalidate('doc::');
	assert.equal(cache.get('doc::'), undefined);
	assert.equal(cache.get('doc::sibling')?.data, 2);

	const circular = {};
	circular.self = circular;
	assert.throws(() => cacheKey(TODOS, circular), /circular references/);
});

test('cache operations roll back and pending list merges preserve local projection gaps', () => {
	const cache = new QueryCache();
	const key = cacheKey(TODOS);
	cache.set(key, {
		data: { todos: [{ id: 'a', title: 'original', status: 'open' }] },
		updatedAt: 1
	});

	const snapshots = applyCacheOps(cache, [
		{
			op: 'upsert',
			target: { document: TODOS, at: 'todos', by: 'id' },
			row: { id: 'b', title: 'optimistic', status: 'open' }
		},
		{
			op: 'patch',
			target: { document: TODOS, at: 'todos', by: 'id' },
			row: { id: 'a', status: 'done' }
		}
	]);
	assert.deepEqual(cache.get(key)?.data.todos, [
		{ id: 'a', title: 'original', status: 'done' },
		{ id: 'b', title: 'optimistic', status: 'open' }
	]);
	rollback(cache, snapshots);
	assert.deepEqual(cache.get(key)?.data.todos, [
		{ id: 'a', title: 'original', status: 'open' }
	]);

	cache.set(key, {
		data: { todos: [{ id: 'a', title: 'local', status: 'done' }] },
		updatedAt: 2,
		pending: true,
		optimistic: true
	});
	writeServerDataPreservingPending(
		cache,
		TODOS,
		undefined,
		{ todos: [{ id: 'a', title: 'stale', status: 'open' }] },
		{ list: { at: 'todos', by: 'id' } }
	);
	assert.deepEqual(cache.get(key)?.data.todos, [
		{ id: 'a', title: 'local', status: 'done' }
	]);
	assert.equal(cache.get(key)?.pending, true);

	writeServerDataPreservingPending(
		cache,
		TODOS,
		undefined,
		{ todos: [{ id: 'a', title: 'local', status: 'done' }] },
		{ list: { at: 'todos', by: 'id' } }
	);
	assert.equal(cache.get(key)?.pending, false);
	assert.equal(cache.get(key)?.optimistic, false);
});
