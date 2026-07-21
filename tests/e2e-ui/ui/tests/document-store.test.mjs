/**
 * Document store: cache is transparent (seed + follow + live write-through).
 * The runtime is imported only through the package's public entry points.
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createDocumentStore } from '@hops-ops/distributed';
import { QueryCache, cacheKey } from '@hops-ops/distributed/cache';

const uiRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

test('createDocumentStore seeds cache and updates when cache changes', async () => {
  const document = 'query Q { items { id } }';
  const cache = new QueryCache();
  const client = {
    cache,
    async request() {
      return { data: { items: [{ id: '2' }] }, status: 200 };
    },
    subscribe() {
      return () => {};
    }
  };

  const store = createDocumentStore(client, {
    document,
    initialData: { items: [{ id: '1' }] },
    select: (data) => data.items
  });

  let latest;
  const unsubscribe = store.subscribe((snapshot) => {
    latest = snapshot;
  });
  assert.equal(latest?.data[0]?.id, '1', 'initial seed failed');

  cache.set(cacheKey(document, {}), {
    data: { items: [{ id: '1' }, { id: 'x' }] },
    updatedAt: Date.now()
  });
  assert.equal(latest.data.length, 2, 'cache notification failed');

  await store.refetch();
  assert.equal(latest.data[0]?.id, '2', 'refetch did not update store');

  unsubscribe();
  store.destroy();
});

test('pages use gql.store / gql.live — no manual cache plumbing', () => {
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
