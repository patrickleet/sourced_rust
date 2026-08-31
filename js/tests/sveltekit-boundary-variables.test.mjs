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

const BoundTodosArtifact = Object.freeze({
	...TodosArtifact,
	id: 'bound-todos-v1',
	document:
		'query BoundTodos($id: ID!, $limit: Int!, $nil: String, $omitted: String, $owner: String, $payload: JSON, $tags: [String!]) { todos { id title status } }',
	protocol: Object.freeze({
		...TodosArtifact.protocol,
		operation: 'bound-todos-v1'
	}),
	variableCodec: Object.freeze({
		version: 1,
		limits: Object.freeze({ maxDepth: 64, maxBoolWidth: 256, maxInList: 1000 }),
		variables: Object.freeze({
			id: Object.freeze({
				kind: 'scalar', scalar: 'ID', codec: 'string', nullable: false
			}),
			limit: Object.freeze({
				kind: 'scalar', scalar: 'Int', codec: 'int32', nullable: false
			}),
			nil: Object.freeze({
				kind: 'scalar', scalar: 'String', codec: 'string', nullable: true
			}),
			omitted: Object.freeze({
				kind: 'scalar', scalar: 'String', codec: 'string', nullable: true
			}),
			owner: Object.freeze({
				kind: 'scalar', scalar: 'String', codec: 'string', nullable: true
			}),
			payload: Object.freeze({
				kind: 'scalar', scalar: 'JSON', codec: 'json', nullable: true
			}),
			tags: Object.freeze({
				kind: 'list',
				nullable: true,
				item: Object.freeze({
					kind: 'scalar', scalar: 'String', codec: 'string', nullable: false
				})
			})
		}),
		inputs: Object.freeze({})
	})
});

function binding(limit = 20) {
	return defineDistributedBoundaryBinding(BoundTodosArtifact, {
		id: { kind: 'route_param', name: 'itemId' },
		limit: { kind: 'constant', value: limit },
		nil: { kind: 'constant', value: null },
		omitted: { kind: 'omit' },
		owner: { kind: 'trusted_session', path: ['user', 'id'] },
		payload: { kind: 'forwarded_prop', path: ['filters'] },
		tags: { kind: 'search_param', name: 'tag', mode: 'all' }
	});
}

function operation(bound = binding()) {
	return defineDistributedBoundaryOperation(
		{
			operation: 'BoundTodos',
			route: '/items/[itemId]',
			kind: 'page',
			discovery: 'component',
			sourcePath: 'src/lib/ItemList.graphql'
		},
		BoundTodosArtifact,
		bound
	);
}

function operationAt({
	name,
	route,
	kind,
	limit
}) {
	return defineDistributedBoundaryOperation(
		{
			operation: name,
			route,
			kind,
			discovery: 'explicit'
		},
		BoundTodosArtifact,
		binding(limit)
	);
}

function context() {
	return Object.freeze({
		params: Object.freeze({ itemId: 'café/東京' }),
		search: new URLSearchParams('tag=%CE%B2&tag=%CE%B1'),
		session: Object.freeze({ user: Object.freeze({ id: 'owner-1' }) }),
		props: Object.freeze({
			filters: Object.freeze({ z: 2, a: Object.freeze(['雪', 1]) })
		})
	});
}

function jsonResponse(body) {
	return {
		status: 200,
		statusText: 'OK',
		text: async () => JSON.stringify(body)
	};
}

test('one boundary binding canonicalizes route, search, session, constant, and forwarded props', () => {
	const first = binding();
	const variables = first.resolve(context());
	assert.deepEqual(variables, {
		id: 'café/東京',
		limit: 20,
		nil: null,
		owner: 'owner-1',
		payload: { a: ['雪', 1], z: 2 },
		tags: ['β', 'α']
	});
	assert.equal(Object.hasOwn(variables, 'omitted'), false, 'omission remains distinct from null');
	assert.equal(first.canonicalBytes(context()), JSON.stringify(variables));
	assert.equal(binding().id, first.id, 'equivalent bindings have stable fingerprints');
	assert.notEqual(binding(21).id, first.id, 'binding contract changes alter hydration identity');
});

test('boundary binding fails closed for missing values, hostile keys, accessors, and forgery paths', () => {
	assert.throws(
		() =>
			defineDistributedBoundaryBinding(BoundTodosArtifact, {
				limit: { kind: 'constant', value: 20 }
			}),
		/missing required variable id/
	);
	assert.throws(
		() =>
			defineDistributedBoundaryBinding(BoundTodosArtifact, {
				id: { kind: 'route_param', name: 'itemId' },
				limit: { kind: 'constant', value: 20 },
				owner: { kind: 'trusted_session', path: ['__proto__', 'owner'] }
			}),
		/invalid path/
	);
	const hostile = Object.create(null);
	Object.defineProperty(hostile, '__proto__', {
		value: 'forged', enumerable: true
	});
	assert.throws(
		() =>
			defineDistributedBoundaryBinding(BoundTodosArtifact, {
				id: { kind: 'route_param', name: 'itemId' },
				limit: { kind: 'constant', value: 20 },
				payload: { kind: 'constant', value: hostile }
			}),
		/hostile object key/
	);
	const params = {};
	Object.defineProperty(params, 'itemId', { get: () => 'secret', enumerable: true });
	assert.throws(
		() => binding().resolve({ ...context(), params }),
		/contains an accessor/
	);
	assert.throws(
		() => binding().resolve({ ...context(), params: {} }),
		/required variable is missing/
	);
});

test('SSR, hydration, component use, navigation read, and prefetch share canonical boundary variables', async () => {
	const boundary = operation();
	const sent = [];
	const server = createDistributedSvelteKitServer({
		boundaries: [boundary],
		getSession: async (event) => event.locals.session,
		getRole: () => 'user'
	});
	const serverContext = context();
	const page = await server.load({
		locals: { session: serverContext.session },
		route: { id: '/items/[itemId]' },
		params: serverContext.params,
		url: new URL(`https://app.example/items/value?${serverContext.search}`),
		parent: async () => serverContext.props,
		async fetch(_url, init) {
			const body = JSON.parse(init.body);
			sent.push(body.variables);
			assert.deepEqual(body.extensions.distributed.client, {
				surface: { kind: 'role', name: 'user' },
				schemaHash: REACT_FIXTURE_SCHEMA
			});
			return jsonResponse(
				todoFrame(
					BoundTodosArtifact,
					[{ id: 'todo-1', title: 'bound', status: 'open' }],
					{ cacheScope: 'cache:owner-1', position: '1' }
				)
			);
		}
	});
	assert.equal(page.gqlError, null);
	assert.deepEqual(page.distributed.bindings, [boundary.binding.id]);
	assert.deepEqual(sent, [boundary.binding.resolve(serverContext)]);

	let browserFetches = 0;
	const client = createDistributedSvelteKit({
		session: { getAuth: () => ({ accessToken: 'browser' }) },
		boundaries: [boundary],
		hydration: page.distributed,
		authority: page.distributedAuthority,
		fetch: async () => {
			browserFetches += 1;
			throw new Error('complete hydrated boundary must not refetch');
		},
		browser: false
	});
	const bound = client.boundary(boundary);
	const lifecycleBytes = [
		boundary.binding.canonicalBytes(serverContext),
		JSON.stringify(bound.variables(serverContext)),
		JSON.stringify(sent[0])
	];
	assert.equal(new Set(lifecycleBytes).size, 1);
	assert.equal(bound.read(serverContext).status, 'ready');
	assert.equal(bound.use(serverContext, { live: false }).get().status, 'ready');
	await bound.prefetch(serverContext);
	assert.equal(browserFetches, 0);
	client.destroy();

	const changedBoundary = operation(binding(21));
	const stale = createDistributedSvelteKit({
		session: { getAuth: () => ({ accessToken: 'browser' }) },
		boundaries: [changedBoundary],
		hydration: page.distributed,
		authority: page.distributedAuthority,
		browser: false
	});
	assert.equal(stale.replica.scope, undefined, 'changed binding rejects stale hydration');
	stale.destroy();
});

test('boundary SSR selects parent layouts, deduplicates exact work, and bounds distinct refreshes', async () => {
	const boundaries = [
		operationAt({ name: 'RootItems', route: '/', kind: 'layout', limit: 20 }),
		operationAt({
			name: 'ItemPage',
			route: '/items/[itemId]',
			kind: 'page',
			limit: 20
		}),
		operationAt({
			name: 'ItemSibling',
			route: '/items',
			kind: 'layout',
			limit: 21
		}),
		operationAt({
			name: 'ItemSiblingTwo',
			route: '/items',
			kind: 'layout',
			limit: 22
		})
	];
	let active = 0;
	let maximum = 0;
	const calls = [];
	const server = createDistributedSvelteKitServer({
		boundaries,
		maxConcurrency: 2,
		getSession: async (event) => event.locals.session,
		getRole: () => 'user'
	});
	const selectedContext = context();
	const page = await server.load({
		locals: { session: selectedContext.session },
		route: { id: '/items/[itemId]' },
		params: selectedContext.params,
		url: new URL(`https://app.example/items/value?${selectedContext.search}`),
		parent: async () => selectedContext.props,
		async fetch(_url, init) {
			const body = JSON.parse(init.body);
			calls.push(body.variables.limit);
			active += 1;
			maximum = Math.max(maximum, active);
			await new Promise((resolve) => setTimeout(resolve, 10));
			active -= 1;
			return jsonResponse(
				todoFrame(
					BoundTodosArtifact,
					[{ id: `todo-${body.variables.limit}`, title: 'bound', status: 'open' }],
					{ cacheScope: 'cache:owner-1', position: String(body.variables.limit) }
				)
			);
		}
	});
	assert.deepEqual(calls.sort((left, right) => left - right), [20, 21, 22]);
	assert.equal(maximum, 2, 'request scheduling never exceeds configured concurrency');
	assert.ok(page.distributed, 'one request-local replica publishes one final seed');
	assert.equal(page.distributed.operations.length, 4, 'logical duplicate ownership remains inspectable');
	assert.equal(
		new Set(page.distributed.bindings).size,
		3,
		'exact duplicate binding identities are represented once'
	);
	assert.equal(page.distributed.bindings.length, 3);
});

test('boundary SSR preserves successful siblings on partial failure and aborts held work', async () => {
	const boundaries = [
		operationAt({ name: 'First', route: '/items', kind: 'layout', limit: 20 }),
		operationAt({ name: 'Second', route: '/items/[itemId]', kind: 'page', limit: 21 })
	];
	const selectedContext = context();
	const server = createDistributedSvelteKitServer({
		boundaries,
		getSession: async (event) => event.locals.session,
		getRole: () => 'user'
	});
	const sharedEvent = {
		locals: { session: selectedContext.session },
		route: { id: '/items/[itemId]' },
		params: selectedContext.params,
		url: new URL(`https://app.example/items/value?${selectedContext.search}`),
		parent: async () => selectedContext.props
	};
	const partial = await server.load({
		...sharedEvent,
		async fetch(_url, init) {
			const body = JSON.parse(init.body);
			if (body.variables.limit === 21) throw new Error('sensitive upstream detail');
			return jsonResponse(
				todoFrame(
					BoundTodosArtifact,
					[{ id: 'todo-20', title: 'valid sibling', status: 'open' }],
					{ cacheScope: 'cache:owner-1', position: '20' }
				)
			);
		}
	});
	assert.ok(partial.distributed, 'successful sibling data remains dehydrated');
	assert.equal(partial.gqlError, 'Distributed GraphQL island refresh failed');
	assert.doesNotMatch(partial.gqlError, /sensitive upstream detail/);

	const controller = new AbortController();
	let transportAborted = false;
	const held = server.load({
		...sharedEvent,
		request: { signal: controller.signal },
		fetch(_url, init) {
			return new Promise((_resolve, reject) => {
				init.signal.addEventListener(
					'abort',
					() => {
						transportAborted = true;
						const error = new Error('transport aborted');
						error.name = 'AbortError';
						reject(error);
					},
					{ once: true }
				);
			});
		}
	});
	await new Promise((resolve) => setTimeout(resolve, 0));
	controller.abort();
	await assert.rejects(held, (error) => error.name === 'AbortError');
	assert.equal(transportAborted, true);
});
