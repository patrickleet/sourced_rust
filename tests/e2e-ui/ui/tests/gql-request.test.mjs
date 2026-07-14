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
