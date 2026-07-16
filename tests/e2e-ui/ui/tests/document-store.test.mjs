/**
 * Document store: cache is transparent (seed + follow + live write-through).
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { spawnSync } from 'node:child_process';

const uiRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

test('createDocumentStore seeds cache and updates when cache changes', () => {
  const script = `
    import { QueryCache, cacheKey } from ${JSON.stringify(
      pathToFileURL(path.join(uiRoot, 'src/lib/gql/cache/query-cache.ts')).href
    )};
    import { createDocumentStore } from ${JSON.stringify(
      pathToFileURL(path.join(uiRoot, 'src/lib/gql/document-store.ts')).href
    )};

    const DOC = 'query Q { items { id } }';
    const cache = new QueryCache();
    const client = {
      cache,
      async request() { return { data: { items: [{ id: '2' }] } }; },
      subscribe() { return () => {}; }
    };

    const store = createDocumentStore(client, {
      document: DOC,
      initialData: { items: [{ id: '1' }] },
      select: (d) => d.items
    });

    let latest;
    const unsub = store.subscribe((s) => { latest = s; });
    if (!latest || latest.data[0].id !== '1') throw new Error('initial seed failed');

    cache.set(cacheKey(DOC, {}), {
      data: { items: [{ id: '1' }, { id: 'x' }] },
      updatedAt: Date.now()
    });
    if (latest.data.length !== 2) throw new Error('cache notify failed: ' + latest.data.length);

    await store.refetch();
    if (latest.data[0].id !== '2') throw new Error('refetch did not update store');

    unsub();
    store.destroy();
    console.log('document-store-ok');
  `;

  const r = spawnSync(
    process.execPath,
    ['--experimental-strip-types', '--input-type=module', '-e', script],
    { encoding: 'utf8', cwd: uiRoot }
  );
  assert.equal(r.status, 0, `stderr=${r.stderr}\nstdout=${r.stdout}`);
  assert.match(r.stdout, /document-store-ok/);
});

test('pages use gql.store / gql.live — no manual seedQueryCache', () => {
  const todos = fs.readFileSync(path.join(uiRoot, 'src/routes/todos/+page.svelte'), 'utf8');
  assert.match(todos, /gql\.store\s*\(/);
  assert.match(todos, /\$list\.data/);
  assert.doesNotMatch(todos, /seedQueryCache|readQueryList|cache\.subscribe/);

  const chat = fs.readFileSync(path.join(uiRoot, 'src/routes/chat/+page.svelte'), 'utf8');
  assert.match(chat, /gql\.live\s*\(/);
  assert.match(chat, /\$lobby\.(data|status)/);
  assert.doesNotMatch(chat, /seedQueryCache|readQueryList|syncFromCache/);
  assert.doesNotMatch(chat, /gql\.subscribe\s*\(/);

  const admin = fs.readFileSync(path.join(uiRoot, 'src/routes/admin/+page.svelte'), 'utf8');
  assert.match(admin, /gql\.store\s*\(/);
  assert.match(admin, /\$list\.data/);
  assert.doesNotMatch(admin, /seedQueryCache|readQueryList/);
});
