/**
 * Runnable via: node --experimental-strip-types tests/run-pipeline-unit.mjs
 * Imports the package's public cache contract exactly as consumers do.
 */
import assert from 'node:assert/strict';
import {
  QueryCache,
  cacheKey,
  applyCacheOps,
  rollback,
  runCommandPipeline,
  fx
} from '@hops-ops/distributed/cache';

const TODOS_DOC = 'query Todos { todos { todo_id title status } }';
const CREATE_DOC = 'mutation TodosCreate($input: TodosCreateInput!) { todos_create(input: $input) { todo_id } }';

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

check('QueryCache set/get/invalidate', () => {
  const cache = new QueryCache();
  const key = cacheKey(TODOS_DOC, {});
  cache.set(key, { data: { todos: [] }, updatedAt: 1 });
  assert.deepEqual(cache.get(key)?.data, { todos: [] });
  cache.invalidate(key);
  assert.equal(cache.get(key), undefined);
});

check('optimistic upsert into list by PK', () => {
  const cache = new QueryCache();
  const key = cacheKey(TODOS_DOC, {});
  cache.set(key, {
    data: { todos: [{ todo_id: 'a', title: 'old', status: 'open' }] },
    updatedAt: 1
  });
  applyCacheOps(cache, [
    {
      op: 'upsert',
      target: { document: TODOS_DOC, at: 'todos', by: 'todo_id' },
      row: { todo_id: 'b', title: 'new', status: 'open' }
    }
  ]);
  const todos = cache.get(key).data.todos;
  assert.equal(todos.length, 2);
  assert.equal(todos[1].todo_id, 'b');
});

check('rollback restores previous cache', () => {
  const cache = new QueryCache();
  const key = cacheKey(TODOS_DOC, {});
  cache.set(key, {
    data: { todos: [{ todo_id: 'a', title: 'old', status: 'open' }] },
    updatedAt: 1
  });
  const snaps = applyCacheOps(cache, [
    {
      op: 'upsert',
      target: { document: TODOS_DOC, at: 'todos', by: 'todo_id' },
      row: { todo_id: 'b', title: 'ghost', status: 'open' }
    }
  ]);
  assert.equal(cache.get(key).data.todos.length, 2);
  rollback(cache, snaps);
  assert.equal(cache.get(key).data.todos.length, 1);
  assert.equal(cache.get(key).data.todos[0].todo_id, 'a');
});

await checkAsync('error path rolls back optimistic and runs onError effect', async () => {
  const cache = new QueryCache();
  const key = cacheKey(TODOS_DOC, {});
  cache.set(key, { data: { todos: [] }, updatedAt: 1 });
  const effects = [];
  const result = await runCommandPipeline(
    {
      cache,
      request: async () => ({ data: null, errors: [{ message: 'boom' }] }),
      runEffects: (e) => effects.push(...e)
    },
    CREATE_DOC,
    { todo_id: 'x', title: 't' },
    {
      optimistic: {
        targets: [{ document: TODOS_DOC, at: 'todos', by: 'todo_id' }],
        row: { todo_id: 'x', title: 't', status: 'open' }
      },
      result: { kind: 'ack' },
      onError: ({ errors }) => [fx.alert(errors[0].message)]
    }
  );
  assert.ok(result.errors?.length);
  assert.equal(cache.get(key).data.todos.length, 0);
  assert.equal(effects[0]?.kind, 'alert');
  assert.equal(effects[0]?.message, 'boom');
});

await checkAsync('ack success does not invent full list row from incomplete payload', async () => {
  const cache = new QueryCache();
  const key = cacheKey(TODOS_DOC, {});
  cache.set(key, { data: { todos: [] }, updatedAt: 1 });
  // No optimistic — only incomplete server payload
  const result = await runCommandPipeline(
    {
      cache,
      request: async () => ({
        data: { todos_create: { todo_id: 'only-id' } },
        errors: null
      })
    },
    CREATE_DOC,
    { todo_id: 'only-id', title: 't' },
    {
      result: { kind: 'ack' },
      reconcile: { kind: 'none' }
    }
  );
  assert.ok(result.data);
  // Must not invent { todo_id, title, status } list row from ack payload alone
  assert.deepEqual(cache.get(key).data.todos, []);
});

await checkAsync('projection applies payload fields only to targets', async () => {
  const cache = new QueryCache();
  const key = cacheKey(TODOS_DOC, {});
  cache.set(key, {
    data: { todos: [{ todo_id: 'g', title: 'old', status: 'open', secret: 'keep' }] },
    updatedAt: 1
  });
  await runCommandPipeline(
    {
      cache,
      request: async () => ({
        data: {
          game_move: { todo_id: 'g', title: 'from-server', status: 'done' }
        },
        errors: null
      })
    },
    'mutation M($input: In!) { game_move(input: $input) { todo_id title status } }',
    { todo_id: 'g' },
    {
      result: {
        kind: 'projection',
        apply: {
          targets: [{ document: TODOS_DOC, at: 'todos', by: 'todo_id' }]
        }
      }
    }
  );
  const row = cache.get(key).data.todos.find((t) => t.todo_id === 'g');
  assert.equal(row.title, 'from-server');
  assert.equal(row.status, 'done');
  // secret was not in payload — merge keeps existing on upsert merge
  assert.equal(row.secret, 'keep');
});

await checkAsync('onSuccess toast is Effect and not run on error', async () => {
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
      result: { kind: 'ack' },
      onSuccess: () => [fx.toast('Created')],
      onError: () => [fx.alert('Failed')]
    }
  );
  assert.equal(effects.length, 1);
  assert.equal(effects[0].kind, 'alert');
});

await checkAsync('exactly one network request for command', async () => {
  let calls = 0;
  await runCommandPipeline(
    {
      cache: new QueryCache(),
      request: async () => {
        calls += 1;
        return { data: { todos_create: { todo_id: '1' } }, errors: null };
      }
    },
    CREATE_DOC,
    { todo_id: '1' },
    { result: { kind: 'ack' } }
  );
  assert.equal(calls, 1);
});

console.log(`# tests ${passed}`);
console.log(`# pass ${passed}`);
if (process.exitCode) process.exit(process.exitCode);
