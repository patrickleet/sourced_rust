/**
 * Pages use the app-generated generic binder with the package cache/pipeline.
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { spawnSync } from 'node:child_process';

const uiRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

test('generated bindCommands: optimistic create + rollback on error', () => {
  const script = `
    import { QueryCache, cacheKey } from '@hops-ops/distributed/cache';
    import { bindCommands, COMMAND_DOCS } from ${JSON.stringify(
      pathToFileURL(path.join(uiRoot, 'src/lib/api/commands.generated.ts')).href
    )};

    const TODOS = 'query Todos { todos { todo_id title status owner_id } }';
    const cache = new QueryCache();
    const key = cacheKey(TODOS, {});
    cache.set(key, { data: { todos: [] }, updatedAt: 1 });

    let calls = 0;
    const client = {
      async request(document, variables) {
        calls += 1;
        if (String(document).includes('todos_create')) {
          return { data: null, errors: [{ message: 'nope' }], status: 200 };
        }
        return { data: { todos: [] }, status: 200 };
      }
    };

    const commands = bindCommands(client, { cache });
    const r = await commands.todosCreate(
      { todo_id: 't1', title: 'x' },
      {
        // Node unit tests have no window — force browser pipeline path.
        browser: true,
        result: { kind: 'fact' },
        reconcile: { kind: 'none' },
        optimistic: {
          targets: [{ document: TODOS, at: 'todos', by: 'todo_id' }],
          row: { todo_id: 't1', title: 'x', status: 'open', owner_id: 'me' }
        }
      }
    );
    if (!r.errors?.length) throw new Error('expected errors');
    const todos = cache.get(key)?.data?.todos ?? [];
    if (todos.length !== 0) throw new Error('expected rollback, got ' + todos.length);
    if (calls !== 1) throw new Error('expected 1 network call, got ' + calls);
    if (!COMMAND_DOCS.todos_create) throw new Error('missing COMMAND_DOCS');
    console.log('page-pipeline-ok');
  `;

  const r = spawnSync(
    process.execPath,
    ['--experimental-strip-types', '--input-type=module', '-e', script],
    { encoding: 'utf8', cwd: uiRoot }
  );
  assert.equal(r.status, 0, `stderr=${r.stderr}\nstdout=${r.stdout}`);
  assert.match(r.stdout, /page-pipeline-ok/);
});

test('generated bindCommands: fact success reconciles without duplicate rows', () => {
  const script = `
    import { QueryCache, cacheKey } from '@hops-ops/distributed/cache';
    import { bindCommands } from ${JSON.stringify(
      pathToFileURL(path.join(uiRoot, 'src/lib/api/commands.generated.ts')).href
    )};

    const TODOS = 'query Todos { todos { todo_id title status owner_id } }';
    const cache = new QueryCache();
    const key = cacheKey(TODOS, {});
    cache.set(key, { data: { todos: [] }, updatedAt: 1 });

    let calls = 0;
    const client = {
      async request(document, variables) {
        calls += 1;
        if (String(document).includes('mutation')) {
          return {
            data: { todos_create: { todo_id: 't1', owner_id: 'a', title: 'x', status: 'open' } },
            status: 200
          };
        }
        // refetch reconcile
        return {
          data: {
            todos: [{ todo_id: 't1', owner_id: 'a', title: 'x', status: 'open' }]
          },
          status: 200
        };
      }
    };

    const commands = bindCommands(client, { cache });
    const r = await commands.todosCreate(
      { todo_id: 't1', title: 'x' },
      {
        browser: true,
        result: { kind: 'fact' },
        reconcile: { kind: 'refetch', document: TODOS },
        optimistic: {
          targets: [{ document: TODOS, at: 'todos', by: 'todo_id' }],
          row: { todo_id: 't1', title: 'x', status: 'open', owner_id: 'a' }
        }
      }
    );
    if (!r.data || r.data.todo_id !== 't1') throw new Error('unwrap failed');
    const todos = cache.get(key)?.data?.todos ?? [];
    if (todos.length !== 1) throw new Error('expected 1 row after reconcile, got ' + todos.length);
    if (calls < 2) throw new Error('expected command + refetch, got ' + calls);
    console.log('page-pipeline-refetch-ok');
  `;

  const r = spawnSync(
    process.execPath,
    ['--experimental-strip-types', '--input-type=module', '-e', script],
    { encoding: 'utf8', cwd: uiRoot }
  );
  assert.equal(r.status, 0, `stderr=${r.stderr}\nstdout=${r.stdout}`);
  assert.match(r.stdout, /page-pipeline-refetch-ok/);
});
