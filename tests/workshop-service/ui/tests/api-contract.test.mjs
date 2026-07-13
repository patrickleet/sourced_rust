/**
 * UI↔API contract: workshopGraphql helper path + identity headers.
 * Runs without a browser; against WORKSHOP_BASE_URL when set, else skips.
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';

const base = process.env.WORKSHOP_BASE_URL;

test('graphql contract with identity headers', { skip: !base }, async () => {
  const res = await fetch(`${base}/graphql`, {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      'x-user-id': 'admin-1',
      'x-role': 'admin',
    },
    body: JSON.stringify({ query: '{ products { product_id } }' }),
  });
  assert.equal(res.status, 200);
  const body = await res.json();
  assert.ok(body.data, `expected data, got ${JSON.stringify(body)}`);
  assert.ok(Array.isArray(body.data.products));
});

test('graphql helper module is present', async () => {
  const fs = await import('node:fs');
  const path = new URL('../src/lib/server/graphql.ts', import.meta.url);
  const src = fs.readFileSync(path, 'utf8');
  assert.match(src, /x-user-id/);
  assert.match(src, /x-role/);
  assert.match(src, /WORKSHOP_BASE_URL/);
});
