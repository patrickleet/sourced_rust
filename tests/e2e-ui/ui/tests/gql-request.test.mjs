/** Consumer-level tests for the package's HTTP, document, resource, and auth APIs. */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { print } from 'graphql';
import {
  buildAuthHeaders,
  createGraphqlClient,
  defineResource,
  documentToString,
  requestGraphql,
  wsConnectionInitPayload
} from '@hops-ops/distributed';
import { authFromPageData } from '@hops-ops/distributed/sveltekit';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

test('createGraphqlClient binds URL, auth, HTTP transport, and subscriptions', async () => {
  const calls = [];
  const client = createGraphqlClient({
    getUrl: () => '/graphql',
    getAuth: () => ({ accessToken: ' token ' }),
    fetch: async (url, init) => {
      calls.push({ url, init });
      return {
        status: 200,
        json: async () => ({ data: { ping: 'pong' } })
      };
    }
  });

  const result = await client.request('query Ping { ping }', { trace: 'one' });
  assert.equal(result.data?.ping, 'pong');
  assert.equal(result.status, 200);
  assert.equal(calls.length, 1);
  assert.equal(calls[0].url, '/graphql');
  assert.equal(calls[0].init.headers.authorization, 'Bearer token');
  assert.deepEqual(JSON.parse(calls[0].init.body).variables, { trace: 'one' });
  assert.equal(typeof client.subscribe, 'function');
});

test('defineResource preserves query/subscription identity and selection', () => {
  const query = 'query Todos { todos { id } }';
  const subscription = 'subscription TodosLive { todos { id } }';
  const resource = defineResource({
    query,
    subscription,
    select: (data) => data.todos
  });

  assert.equal(resource.query, query);
  assert.equal(resource.subscription, subscription);
  assert.deepEqual(resource.select({ todos: [{ id: 'one' }] }), [{ id: 'one' }]);

  const again = defineResource({ query: resource.query, subscription: resource.subscription });
  assert.equal(again.query, resource.query);
  assert.equal(again.subscription, resource.subscription);
  assert.equal('mutations' in resource, false, 'commands are generated separately from resources');
});

test('SvelteKit page auth and shared HTTP/WS headers prefer Bearer', () => {
  const bearer = authFromPageData({
    accessToken: ' tok ',
    session: { user: { id: 'u1' } },
    engineRole: 'admin'
  });
  assert.equal(bearer.accessToken, ' tok ');
  assert.equal(bearer.userId, undefined);

  const development = authFromPageData({
    accessToken: null,
    session: { user: { id: 'dev' } },
    engineRole: 'admin'
  });
  assert.equal(development.userId, 'dev');
  assert.equal(development.role, 'admin');

  const http = buildAuthHeaders({ accessToken: 'abc', userId: 'ignored', role: 'admin' });
  assert.equal(http.authorization, 'Bearer abc');
  assert.equal(http['x-user-id'], undefined);

  const websocket = wsConnectionInitPayload({ accessToken: 'abc' });
  assert.equal(websocket.authorization, 'Bearer abc');
  assert.equal(websocket.accessToken, 'abc');

  const devHeaders = buildAuthHeaders({ userId: 'dev', role: 'admin' });
  assert.equal(devHeaders['x-user-id'], 'dev');
  assert.equal(devHeaders['x-role'], 'admin');
});

test('todos.resource is defineResource wired to todos.gql generated documents', () => {
  const resourceFile = path.join(root, 'src/routes/todos/todos.resource.ts');
  const source = fs.readFileSync(resourceFile, 'utf8');
  assert.match(source, /defineResource/);
  assert.match(source, /@hops-ops\/distributed/);
  assert.match(source, /export const todos/);
  assert.match(source, /TodosDocument|todos\.generated/);
  assert.doesNotMatch(source, /TodosCreateDocument|mutations:/);

  const gql = fs.readFileSync(path.join(root, 'src/routes/todos/todos.gql'), 'utf8');
  assert.match(gql, /todo_id/);
  assert.match(gql, /owner_id/);
  assert.match(gql, /title/);
  assert.match(gql, /status/);
  assert.doesNotMatch(gql, /mutation /);
});

test('chat.resource is defineResource wired to query and subscription documents', () => {
  const resourceFile = path.join(root, 'src/routes/chat/chat.resource.ts');
  const source = fs.readFileSync(resourceFile, 'utf8');
  assert.match(source, /defineResource/);
  assert.match(source, /@hops-ops\/distributed/);
  assert.match(source, /export const chat/);
  assert.match(source, /ChatMessagesDocument|chat\.generated/);
  assert.match(source, /ChatMessagesLiveDocument/);
  assert.doesNotMatch(source, /ChatPostDocument|mutations:/);
  assert.match(source, /LOBBY_ROOM/);

  const gql = fs.readFileSync(path.join(root, 'src/routes/chat/chat.gql'), 'utf8');
  assert.match(gql, /message_id/);
  assert.match(gql, /room_id/);
  assert.match(gql, /subscription/);
  assert.doesNotMatch(gql, /mutation /);
});

test('documentToString prints TypedDocumentNode-compatible ASTs for the wire', () => {
  const ast = {
    kind: 'Document',
    definitions: [{
      kind: 'OperationDefinition',
      operation: 'query',
      name: { kind: 'Name', value: 'Ping' },
      selectionSet: {
        kind: 'SelectionSet',
        selections: [{ kind: 'Field', name: { kind: 'Name', value: '__typename' } }]
      }
    }]
  };

  const source = documentToString(ast);
  assert.match(source, /__typename/);
  assert.equal(source, print(ast));
  assert.equal(documentToString('raw { x }'), 'raw { x }');
});

test('requestGraphql sends variables and reports both 401 response shapes', async () => {
  const calls = [];
  const okFetch = async (url, init) => {
    calls.push({ url, init });
    return { status: 200, json: async () => ({ data: { ok: true } }) };
  };
  const result = await requestGraphql(
    'http://api.test/graphql',
    '{ __typename }',
    { accessToken: 'abc' },
    { a: 1 },
    { fetch: okFetch }
  );
  assert.equal(result.status, 200);
  assert.equal(result.data?.ok, true);
  assert.equal(calls[0].url, 'http://api.test/graphql');
  assert.deepEqual(JSON.parse(calls[0].init.body), {
    query: '{ __typename }',
    variables: { a: 1 }
  });
  assert.equal(calls[0].init.headers.authorization, 'Bearer abc');

  const rejected = await requestGraphql('/graphql', 'query Q { q }', {}, {}, {
    fetch: async () => ({
      status: 401,
      json: async () => ({ error: 'unauthorized' })
    })
  });
  assert.equal(rejected.status, 401);
  assert.match(rejected.errors?.[0]?.message ?? '', /unauthorized/i);

  const missing = await requestGraphql('/graphql', 'query Q { q }', {}, {}, {
    fetch: async () => ({ status: 401, json: async () => ({}) })
  });
  assert.equal(missing.status, 401);
  assert.match(missing.errors?.[0]?.message ?? '', /access token/i);
});
