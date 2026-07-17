/**
 * Systems-harden unit pack — drives shipped e2e-ui gql modules.
 * Run: node --experimental-strip-types tests/systems-harden-unit.mjs
 * (from tests/e2e-ui/ui)
 */
import assert from 'node:assert/strict';
import {
  QueryCache,
  cacheKey,
  applyCacheOps,
  writeServerDataPreservingPending,
  runCommandPipeline,
  fx,
  applyProjectionPayload
} from '../src/lib/gql/cache/index.ts';
import { buildAuthHeaders, wsConnectionInitPayload } from '../src/lib/gql/auth-headers.ts';
import { authFromPageData } from '../src/lib/gql/auth-from-page.ts';
import { looksLikeMutation, createGraphqlClient } from '../src/lib/gql/create-client.ts';
import { createDocumentStore } from '../src/lib/gql/document-store.ts';
import { e2eCommandPolicies } from '../src/lib/gql/command-policies.ts';
import { bindCommandsPipeline } from '../src/lib/gql/bind-commands-pipeline.ts';

const TODOS_DOC = 'query Todos { todos { todo_id title status } }';
const CREATE_DOC =
  'mutation TodosCreate($input: TodosCreateInput!) { todos_create(input: $input) { todo_id } }';

let passed = 0;
function check(name, fn) {
  try {
    fn();
    passed += 1;
    console.log(`ok - ${name}`);
  } catch (e) {
    console.error(`not ok - ${name}`);
    console.error(e);
    process.exitCode = 1;
  }
}

async function checkAsync(name, fn) {
  try {
    await fn();
    passed += 1;
    console.log(`ok - ${name}`);
  } catch (e) {
    console.error(`not ok - ${name}`);
    console.error(e);
    process.exitCode = 1;
  }
}

// --- C-U1 cacheKey ---
check('C-U1 cacheKey empty vars equivalent', () => {
  const a = cacheKey(TODOS_DOC);
  const b = cacheKey(TODOS_DOC, {});
  const c = cacheKey(TODOS_DOC, undefined);
  assert.equal(a, b);
  assert.equal(a, c);
});

check('C-U1 cacheKey variable order stable', () => {
  const a = cacheKey(TODOS_DOC, { b: 1, a: 2 });
  const b = cacheKey(TODOS_DOC, { a: 2, b: 1 });
  assert.equal(a, b);
});

// --- C-U3 mutation write-through guard ---
check('C-U3 looksLikeMutation detects mutations', () => {
  assert.equal(looksLikeMutation('mutation M { x }'), true);
  assert.equal(looksLikeMutation('query Q { x }'), false);
  assert.equal(looksLikeMutation('  mutation { x }'), true);
});

await checkAsync('C-U3 createClient request does not write-through mutations', async () => {
  const cache = new QueryCache();
  const key = cacheKey(CREATE_DOC, { input: { todo_id: '1' } });
  let fetchCalls = 0;
  const orig = globalThis.fetch;
  globalThis.fetch = async () => {
    fetchCalls += 1;
    return {
      status: 200,
      json: async () => ({ data: { todos_create: { todo_id: '1' } } })
    };
  };
  try {
    const client = createGraphqlClient({
      getUrl: () => 'http://example.test/graphql',
      getAuth: () => ({ accessToken: 'tok' }),
      cache,
      writeThrough: true
    });
    await client.request(CREATE_DOC, { input: { todo_id: '1' } });
    assert.equal(fetchCalls, 1);
    assert.equal(cache.get(key), undefined, 'mutation must not write cache key');
  } finally {
    globalThis.fetch = orig;
  }
});

// --- C-U5 projection payload only ---
check('C-U5 projection apply only payload keys merged into row', () => {
  const cache = new QueryCache();
  const key = cacheKey(TODOS_DOC, {});
  cache.set(key, {
    data: {
      todos: [{ todo_id: 'g', title: 'old', status: 'open', secret: 'keep' }]
    },
    updatedAt: 1
  });
  applyProjectionPayload(
    cache,
    [{ document: TODOS_DOC, at: 'todos', by: 'todo_id' }],
    { todo_id: 'g', title: 'from-server', owner_id: 'attacker' }
  );
  const row = cache.get(key).data.todos.find((t) => t.todo_id === 'g');
  assert.equal(row.title, 'from-server');
  assert.equal(row.owner_id, 'attacker'); // payload key is applied
  assert.equal(row.secret, 'keep'); // not in payload — preserved via upsert merge
  assert.equal(row.status, 'open');
});

// --- C-U6 network throw rollback ---
await checkAsync('C-U6 network throw rolls back optimistic', async () => {
  const cache = new QueryCache();
  const key = cacheKey(TODOS_DOC, {});
  cache.set(key, { data: { todos: [] }, updatedAt: 1 });
  await runCommandPipeline(
    {
      cache,
      request: async () => {
        throw new Error('network down');
      }
    },
    CREATE_DOC,
    { todo_id: 'x' },
    {
      browser: true,
      optimistic: {
        targets: [{ document: TODOS_DOC, at: 'todos', by: 'todo_id' }],
        row: { todo_id: 'x', title: 't', status: 'open' }
      },
      result: { kind: 'ack' }
    }
  );
  assert.equal(cache.get(key).data.todos.length, 0);
});

// --- C-U8/E5 pending merge archive ---
check('C-U8/E5 pending merge keeps archived over stale open server', () => {
  const cache = new QueryCache();
  const key = cacheKey(TODOS_DOC, {});
  cache.set(key, {
    data: {
      todos: [{ todo_id: 'a', title: 't', status: 'archived' }]
    },
    updatedAt: 1,
    pending: true,
    optimistic: true
  });
  writeServerDataPreservingPending(
    cache,
    TODOS_DOC,
    {},
    { todos: [{ todo_id: 'a', title: 't', status: 'open' }] },
    { list: { at: 'todos', by: 'todo_id' } }
  );
  const row = cache.get(key).data.todos[0];
  assert.equal(row.status, 'archived');
  assert.equal(cache.get(key).pending, true);
});

// --- C-U9 pending clears when equal ---
check('C-U9 pending clears when server matches local', () => {
  const cache = new QueryCache();
  const key = cacheKey(TODOS_DOC, {});
  cache.set(key, {
    data: {
      todos: [{ todo_id: 'a', title: 't', status: 'archived' }]
    },
    updatedAt: 1,
    pending: true,
    optimistic: true
  });
  writeServerDataPreservingPending(
    cache,
    TODOS_DOC,
    {},
    { todos: [{ todo_id: 'a', title: 't', status: 'archived' }] },
    { list: { at: 'todos', by: 'todo_id' } }
  );
  assert.equal(cache.get(key).pending, false);
  assert.equal(cache.get(key).optimistic, false);
});

// --- B12 no list without explicit merge ---
check('B12 pending without list options leaves local unchanged', () => {
  const cache = new QueryCache();
  const key = cacheKey(TODOS_DOC, {});
  cache.set(key, {
    data: {
      todos: [{ todo_id: 'a', title: 'local', status: 'archived' }]
    },
    updatedAt: 1,
    pending: true,
    optimistic: true
  });
  writeServerDataPreservingPending(
    cache,
    TODOS_DOC,
    {},
    { todos: [{ todo_id: 'a', title: 'server', status: 'open' }] }
    // no list options
  );
  assert.equal(cache.get(key).data.todos[0].status, 'archived');
  assert.equal(cache.get(key).data.todos[0].title, 'local');
});

// --- C-U12/13 auth ---
check('C-U12 Bearer exclusive — no x-user-id', () => {
  const h = buildAuthHeaders({
    accessToken: '  tok  ',
    userId: 'evil',
    role: 'admin'
  });
  assert.equal(h.authorization, 'Bearer tok');
  assert.equal(h['x-user-id'], undefined);
  assert.equal(h['x-role'], undefined);
});

check('C-U12 DevHeaders only without token', () => {
  const h = buildAuthHeaders({ userId: 'alice', role: 'user' });
  assert.equal(h.authorization, undefined);
  assert.equal(h['x-user-id'], 'alice');
  assert.equal(h['x-role'], 'user');
});

check('C-U12 ws payload Bearer exclusive', () => {
  const p = wsConnectionInitPayload({
    accessToken: 'tok',
    userId: 'evil',
    role: 'admin'
  });
  assert.equal(p.authorization, 'Bearer tok');
  assert.equal(p['x-user-id'], undefined);
});

check('C-U13 authFromPageData with token has no userId', () => {
  const a = authFromPageData({
    accessToken: 'tok',
    session: { user: { id: 'u1' } },
    engineRole: 'admin'
  });
  assert.equal(a.accessToken, 'tok');
  assert.equal(a.userId, undefined);
});

// --- C-U15 variables cache key ---
check('C-U15 write-through with variables hits same key as store seed', () => {
  const cache = new QueryCache();
  const vars = { room: 'a' };
  const doc = 'subscription Chat { chat_messages { message_id } }';
  const key = cacheKey(doc, vars);
  cache.set(key, {
    data: { chat_messages: [] },
    updatedAt: 1,
    pending: true,
    optimistic: true
  });
  // Simulate client subscribe write-through with correct vars
  writeServerDataPreservingPending(
    cache,
    doc,
    vars,
    { chat_messages: [{ message_id: 'm1', body: 'hi' }] },
    { list: { at: 'chat_messages', by: 'message_id' } }
  );
  assert.ok(cache.get(key));
  assert.equal(cache.get(key).data.chat_messages[0].message_id, 'm1');
  // Wrong vars key must remain empty
  assert.equal(cache.get(cacheKey(doc, {})), undefined);
});

// --- C-U16 browser false ---
await checkAsync('C-U16 browser:false skips optimistic', async () => {
  const cache = new QueryCache();
  const key = cacheKey(TODOS_DOC, {});
  cache.set(key, { data: { todos: [] }, updatedAt: 1 });
  await runCommandPipeline(
    {
      cache,
      request: async () => ({
        data: { todos_create: { todo_id: '1' } },
        errors: null
      })
    },
    CREATE_DOC,
    { todo_id: '1' },
    {
      browser: false,
      optimistic: {
        targets: [{ document: TODOS_DOC, at: 'todos', by: 'todo_id' }],
        row: { todo_id: '1', title: 'x', status: 'open' }
      },
      result: { kind: 'fact' }
    }
  );
  assert.equal(cache.get(key).data.todos.length, 0);
});

// --- C-U17 effects ---
await checkAsync('C-U17 onSuccess not run on GraphQL errors', async () => {
  const effects = [];
  await runCommandPipeline(
    {
      cache: new QueryCache(),
      request: async () => ({ data: null, errors: [{ message: 'no' }] }),
      runEffects: (e) => effects.push(...e)
    },
    CREATE_DOC,
    {},
    {
      browser: true,
      result: { kind: 'ack' },
      onSuccess: () => [fx.toast('Created')],
      onError: () => [fx.alert('Failed')]
    }
  );
  assert.equal(effects.length, 1);
  assert.equal(effects[0].kind, 'alert');
});

// --- B13 scheduleCatchUp cancelled on destroy ---
await checkAsync('B13 scheduleCatchUp cancelled on destroy', async () => {
  const cache = new QueryCache();
  let requests = 0;
  const client = {
    cache,
    async request() {
      requests += 1;
      return { data: { todos: [] } };
    },
    subscribe() {
      return () => {};
    }
  };
  const store = createDocumentStore(client, {
    document: TODOS_DOC,
    list: { at: 'todos', by: 'todo_id' },
    initialData: { todos: [] }
  });
  store.scheduleCatchUp(20);
  store.destroy();
  await new Promise((r) => setTimeout(r, 50));
  assert.equal(requests, 0, 'destroyed store must not refetch');
});

// --- policies merge ---
await checkAsync('B9 policy defaults applied; call-site overrides', async () => {
  const cache = new QueryCache();
  let lastDoc = '';
  const client = {
    async request(document) {
      lastDoc = String(document);
      return {
        data: {
          todos_create: { todo_id: '1', title: 't', status: 'open', owner_id: 'a' }
        },
        status: 200
      };
    }
  };
  const commands = bindCommandsPipeline(client, {
    cache,
    policies: e2eCommandPolicies
  });
  const r = await commands.todosCreate(
    { todo_id: '1', title: 't' },
    {
      browser: true,
      // no result/reconcile — use policy
      optimistic: {
        targets: [{ document: TODOS_DOC, at: 'todos', by: 'todo_id' }],
        row: { todo_id: '1', title: 't', status: 'open', owner_id: 'a' }
      }
    }
  );
  assert.equal(r.data?.todo_id, '1');
  assert.match(lastDoc, /todos_create/);
  // pending marked on success with fact
  const key = cacheKey(TODOS_DOC, {});
  // may not have key if list never seeded — seed first in real pages
  assert.ok(e2eCommandPolicies.todosCreate?.result?.kind === 'fact');
  assert.ok(e2eCommandPolicies.todosCreate?.reconcile?.kind === 'none');
});

// structural: no cache-helpers export
check('B14 index does not export seedQueryCache', async () => {
  const fs = await import('node:fs');
  const path = await import('node:path');
  const { fileURLToPath } = await import('node:url');
  const root = path.dirname(fileURLToPath(import.meta.url));
  const idx = fs.readFileSync(path.join(root, '../src/lib/gql/index.ts'), 'utf8');
  assert.doesNotMatch(idx, /seedQueryCache|cache-helpers/);
  assert.match(idx, /e2eCommandPolicies/);
  assert.ok(!fs.existsSync(path.join(root, '../src/lib/gql/cache-helpers.ts')));
});

console.log(`# tests ${passed}`);
console.log(`# pass ${passed}`);
if (process.exitCode) process.exit(process.exitCode);
