/**
 * Unit tests for the unified GraphQL request path (real shipped request.ts).
 * Loads request.ts with node --experimental-strip-types (type imports erase).
 * createGraphqlClient is covered by importing its source contract + exercising
 * the same factory pattern against requestGraphql (create-client is a 10-line
 * wrapper around requestGraphql — we assert the file calls it and run the
 * factory pattern on the real requestGraphql entry).
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '..');
const requestFile = path.join(root, 'src/lib/gql/request.ts');
const createFile = path.join(root, 'src/lib/gql/create-client.ts');
const requestUrl = pathToFileURL(requestFile).href;

test('create-client.ts is a thin factory over requestGraphql', () => {
  const src = fs.readFileSync(createFile, 'utf8');
  assert.match(src, /export function createGraphqlClient/);
  assert.match(src, /requestGraphql/);
  assert.match(src, /getUrl/);
  assert.match(src, /getAuth/);
});

const defineFile = path.join(root, 'src/lib/gql/define-resource.ts');
const authFile = path.join(root, 'src/lib/gql/auth-from-page.ts');
const useFile = path.join(root, 'src/lib/gql/use-graphql.ts');
const resourceFile = path.join(root, 'src/routes/todos/todos.resource.ts');
const defineUrl = pathToFileURL(defineFile).href;
const authUrl = pathToFileURL(authFile).href;

test('defineResource preserves document identity for query + mutations', () => {
  const script = `
    import { defineResource } from ${JSON.stringify(defineUrl)};

    const Q = '{ todos { id } }';
    const C = 'mutation { create }';
    const r = defineResource({
      query: Q,
      mutations: { create: C, complete: 'mutation { complete }' },
      select: (d) => d.todos
    });
    if (r.query !== Q) throw new Error('query identity broken');
    if (r.mutations.create !== C) throw new Error('mutation identity broken');
    if (typeof r.select !== 'function') throw new Error('select missing');
    // Same reference when co-located resource reuses defineResource
    const again = defineResource({ query: r.query, mutations: r.mutations });
    if (again.query !== r.query) throw new Error('re-wrap broke query ref');
    if (again.mutations.create !== r.mutations.create) throw new Error('re-wrap broke mut ref');
    console.log('define-resource-ok');
  `;
  const r = spawnSync(
    process.execPath,
    ['--experimental-strip-types', '--input-type=module', '-e', script],
    { encoding: 'utf8' }
  );
  assert.equal(r.status, 0, `stderr=${r.stderr}\nstdout=${r.stdout}`);
  assert.match(r.stdout, /define-resource-ok/);
});

test('authFromPageData + defineResource + requestGraphql is the browser mutation path', () => {
  // useGraphql is createGraphqlClient({ getUrl: '/graphql', getAuth: authFromPageData }).
  // Node cannot resolve extensionless relative imports in create-client; drive the
  // same shipped requestGraphql + authFromPageData + defineResource entry points.
  const useSrc = fs.readFileSync(useFile, 'utf8');
  assert.match(useSrc, /export function useGraphql/);
  assert.match(useSrc, /createGraphqlClient/);
  assert.match(useSrc, /authFromPageData/);
  assert.match(useSrc, /['"]\/graphql['"]/);

  const script = `
    import { authFromPageData } from ${JSON.stringify(authUrl)};
    import { requestGraphql } from ${JSON.stringify(requestUrl)};
    import { defineResource } from ${JSON.stringify(defineUrl)};

    const a = authFromPageData({
      accessToken: ' tok ',
      session: { user: { id: 'u1' } },
      engineRole: 'user'
    });
    if (a.accessToken !== ' tok ') throw new Error('token');
    if (a.userId !== undefined) throw new Error('userId should be omitted when token set');

    const b = authFromPageData({
      accessToken: null,
      session: { user: { id: 'dev' } },
      engineRole: 'admin'
    });
    if (b.userId !== 'dev' || b.role !== 'admin') throw new Error(JSON.stringify(b));

    const Q = '{ todos { todo_id } }';
    const CREATE = 'mutation TodosCreate { todos_create { todo_id } }';
    const resource = defineResource({
      query: Q,
      mutations: { create: CREATE, complete: 'c', archive: 'a' }
    });

    const calls = [];
    globalThis.fetch = async (url, init) => {
      calls.push({ url, body: JSON.parse(init.body), headers: init.headers });
      return {
        status: 200,
        json: async () => ({ data: { todos: [{ todo_id: '1' }], todos_create: { todo_id: '1' } } })
      };
    };

    // Same wiring as useGraphql / createGraphqlClient
    const clientRequest = async (document, variables = {}) => {
      const auth = authFromPageData({ accessToken: 'abc', engineRole: 'user' });
      return requestGraphql('/graphql', document, auth, variables);
    };

    const seed = await clientRequest(resource.query);
    if (!seed.data?.todos) throw new Error(JSON.stringify(seed));
    const mut = await clientRequest(resource.mutations.create, { todo_id: '1', title: 'x' });
    if (!mut.data?.todos_create) throw new Error(JSON.stringify(mut));
    if (calls[0].url !== '/graphql' || calls[0].body.query !== Q) throw new Error('query path');
    if (calls[1].body.query !== CREATE) throw new Error('mutation path');
    if (calls[0].body.query !== resource.query) throw new Error('SSR/client query identity');
    if (calls[0].headers.authorization !== 'Bearer abc') throw new Error('bearer');
    console.log('use-graphql-ok');
  `;
  const r = spawnSync(
    process.execPath,
    ['--experimental-strip-types', '--input-type=module', '-e', script],
    { encoding: 'utf8' }
  );
  assert.equal(r.status, 0, `stderr=${r.stderr}\nstdout=${r.stdout}`);
  assert.match(r.stdout, /use-graphql-ok/);
});

test('todos.resource is defineResource with create/complete/archive + query', () => {
  const src = fs.readFileSync(resourceFile, 'utf8');
  assert.match(src, /defineResource/);
  assert.match(src, /export const todos/);
  assert.match(src, /todos_create/);
  assert.match(src, /todos_complete/);
  assert.match(src, /todos_archive/);
  // Query selection includes todo fields used by UI
  assert.match(src, /todo_id/);
  assert.match(src, /owner_id/);
  assert.match(src, /title/);
  assert.match(src, /status/);
});

test('requestGraphql drives real fetch with Bearer, variables, and 401 path', () => {
  const script = `
    import { requestGraphql, buildAuthHeaders } from ${JSON.stringify(requestUrl)};

    const h = buildAuthHeaders({ accessToken: ' tok ' });
    if (h.authorization !== 'Bearer tok') throw new Error('auth header: ' + h.authorization);
    const h2 = buildAuthHeaders({ userId: 'u1', role: 'admin' });
    if (h2['x-user-id'] !== 'u1' || h2['x-role'] !== 'admin') throw new Error('dev headers');

    const calls = [];
    globalThis.fetch = async (url, init) => {
      calls.push({ url, init });
      return { status: 200, json: async () => ({ data: { ok: true } }) };
    };
    const r = await requestGraphql(
      'http://api.test/graphql',
      '{ __typename }',
      { accessToken: 'abc' },
      { a: 1 }
    );
    if (r.status !== 200 || !r.data?.ok) throw new Error(JSON.stringify(r));
    if (calls[0].url !== 'http://api.test/graphql') throw new Error('url');
    const body = JSON.parse(calls[0].init.body);
    if (body.query !== '{ __typename }' || body.variables.a !== 1) throw new Error('body');
    if (calls[0].init.headers.authorization !== 'Bearer abc') throw new Error('bearer');

    globalThis.fetch = async () => ({
      status: 401,
      json: async () => ({ error: 'unauthorized' })
    });
    const u = await requestGraphql('/graphql', 'q', {});
    const msg = String(u.errors?.[0]?.message ?? '');
    if (u.status !== 401 || (!msg.includes('access token') && !msg.includes('unauthorized') && !msg.includes('Bearer'))) {
      throw new Error(JSON.stringify(u));
    }
    // missing token path (no body.error)
    globalThis.fetch = async () => ({ status: 401, json: async () => ({}) });
    const m = await requestGraphql('/graphql', 'q', {});
    if (m.status !== 401 || !String(m.errors?.[0]?.message).includes('access token')) {
      throw new Error('missing-token: ' + JSON.stringify(m));
    }

    // Factory pattern used by createGraphqlClient (same shipped requestGraphql)
    const createGraphqlClient = (opts) => ({
      request: async (document, variables = {}) => {
        const auth = await opts.getAuth();
        return requestGraphql(opts.getUrl(), document, auth, variables);
      }
    });
    let n = 0;
    globalThis.fetch = async (url) => {
      n++;
      return { status: 200, json: async () => ({ data: { n, url } }) };
    };
    const client = createGraphqlClient({
      getUrl: () => 'http://x/graphql',
      getAuth: () => ({ accessToken: 't' })
    });
    const c = await client.request('{ x }', { z: 2 });
    if (c.data?.url !== 'http://x/graphql' || n !== 1) throw new Error(JSON.stringify(c));
    console.log('gql-request-ok');
  `;

  const r = spawnSync(
    process.execPath,
    ['--experimental-strip-types', '--input-type=module', '-e', script],
    { encoding: 'utf8' }
  );
  assert.equal(r.status, 0, `stderr=${r.stderr}\nstdout=${r.stdout}`);
  assert.match(r.stdout, /gql-request-ok/);
});
