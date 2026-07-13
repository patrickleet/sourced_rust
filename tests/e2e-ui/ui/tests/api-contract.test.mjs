/**
 * UI↔API contract. With E2E_BASE_URL set, hits a live service (no soft-skip).
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';

const base = process.env.E2E_BASE_URL;

test('session + api modules use identity headers', () => {
  const session = fs.readFileSync(new URL('../src/lib/session.ts', import.meta.url), 'utf8');
  assert.match(session, /x-user-id/);
  assert.match(session, /x-role/);
  const api = fs.readFileSync(new URL('../src/lib/api.ts', import.meta.url), 'utf8');
  assert.match(api, /todo\.create/);
  assert.match(api, /listTodos/);
  const ws = fs.readFileSync(new URL('../src/lib/graphql-ws.ts', import.meta.url), 'utf8');
  assert.match(ws, /graphql-transport-ws/);
  assert.match(ws, /subscribe/);
});

test('live create + list isolation', { skip: !base }, async () => {
  const id = `ui-${Date.now().toString(16)}`;
  const create = await fetch(`${base}/todo.create`, {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      'x-user-id': 'alice',
      'x-role': 'user',
    },
    body: JSON.stringify({ todo_id: id, title: 'UI contract todo' }),
  });
  assert.equal(create.status, 200, await create.text());

  // Wait for projection
  let found = false;
  for (let i = 0; i < 40; i++) {
    const res = await fetch(`${base}/graphql`, {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
        'x-user-id': 'alice',
        'x-role': 'user',
      },
      body: JSON.stringify({ query: '{ todos { todo_id owner_id title } }' }),
    });
    assert.equal(res.status, 200);
    const body = await res.json();
    const todos = body.data?.todos ?? [];
    if (todos.some((t) => t.todo_id === id)) {
      found = true;
      assert.ok(todos.every((t) => t.owner_id === 'alice'));
      break;
    }
    await new Promise((r) => setTimeout(r, 50));
  }
  assert.ok(found, 'todo projected for alice');
  console.log(`live UI contract ok todo=${id}`);
});
