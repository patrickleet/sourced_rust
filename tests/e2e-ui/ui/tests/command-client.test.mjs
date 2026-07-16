/**
 * Command client: generator + generated module drive the real client.request path.
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { spawnSync } from 'node:child_process';

const here = path.dirname(fileURLToPath(import.meta.url));
const uiRoot = path.resolve(here, '..');
const genScript = path.join(uiRoot, 'scripts/gen-commands.mjs');
const manifestPath = path.join(uiRoot, 'src/lib/api/commands.manifest.json');
const generatedPath = path.join(uiRoot, 'src/lib/api/commands.generated.ts');
const operationsGql = path.join(uiRoot, 'src/lib/api/commands.operations.gql');
const pageSvelte = path.join(uiRoot, 'src/routes/todos/+page.svelte');

test('commands.manifest.json lists create/complete and admin-only force_archive', () => {
  assert.ok(fs.existsSync(manifestPath), 'manifest missing — run make export-commands');
  const catalog = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
  assert.equal(catalog.version, 1);
  const byField = Object.fromEntries(catalog.commands.map((c) => [c.field_name, c]));
  assert.ok(byField.todos_create, 'todos_create missing');
  assert.ok(byField.todos_complete, 'todos_complete missing');
  assert.ok(byField.todos_force_archive, 'todos_force_archive missing');
  assert.deepEqual(byField.todos_create.roles.slice().sort(), ['admin', 'user']);
  assert.deepEqual(byField.todos_force_archive.roles, ['admin']);
});

test('generateOperationsGql + generateCommandsTs share mutation text', async () => {
  const { generateCommandsTs, generateOperationsGql, fieldToFnName, buildMutationOp } =
    await import(pathToFileURL(genScript).href);
  assert.equal(fieldToFnName('todos_create'), 'todosCreate');
  const catalog = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
  const gql = generateOperationsGql(catalog);
  const ts = generateCommandsTs(catalog);
  assert.match(gql, /mutation Command_todos_create/);
  assert.match(gql, /todos_force_archive/);
  assert.match(ts, /export async function todosCreate/);
  assert.match(ts, /COMMAND_DOCS/);
  assert.match(ts, /CommandClient/);
  // Same op body in both artifacts
  const { text } = buildMutationOp(catalog.commands[0]);
  assert.ok(gql.includes(text));
  assert.ok(ts.includes(text.split('\n')[0]));
});

test('generated todosCreate uses client.request + COMMAND_DOCS', async () => {
  assert.ok(fs.existsSync(generatedPath), 'commands.generated.ts missing');
  assert.ok(fs.existsSync(operationsGql), 'commands.operations.gql missing');

  const script = `
    const mod = await import(${JSON.stringify(pathToFileURL(generatedPath).href)});
    let seen;
    const client = {
      async request(document, variables) {
        seen = { document, variables };
        return {
          data: {
            todos_create: {
              todo_id: 't1',
              owner_id: 'alice',
              title: 'hi',
              status: 'open'
            }
          },
          status: 200
        };
      }
    };
    const r = await mod.todosCreate({ todo_id: 't1', title: 'hi' }, client);
    if (!r.data || r.data.todo_id !== 't1') throw new Error('unwrap failed');
    if (!String(seen.document).includes('todos_create')) throw new Error('doc missing');
    if (seen.variables.input.title !== 'hi') throw new Error('vars');
    if (mod.COMMAND_DOCS.todos_create !== seen.document) throw new Error('COMMAND_DOCS mismatch');
    if (typeof mod.todosComplete !== 'function') throw new Error('todosComplete missing');
    const bound = mod.bindCommands(client);
    const r3 = await bound.todosCreate({ todo_id: 't3', title: 'y' });
    if (!r3.data) throw new Error('bound.todosCreate failed');
    if (!mod.COMMAND_ROLES.todos_force_archive.includes('admin')) throw new Error('roles');
    console.log('command-client-ok');
  `;

  const r = spawnSync(
    process.execPath,
    ['--experimental-strip-types', '--input-type=module', '-e', script],
    { encoding: 'utf8' }
  );
  assert.equal(r.status, 0, `stderr=${r.stderr}\nstdout=${r.stdout}`);
  assert.match(r.stdout, /command-client-ok/);
});

test('app pages use gql.commands.* pipeline (optimistic + fact/ack + reconcile)', () => {
  const todos = fs.readFileSync(pageSvelte, 'utf8');
  assert.match(todos, /gql\.commands\.todosCreate/);
  assert.match(todos, /gql\.commands\.todosComplete/);
  assert.match(todos, /gql\.commands\.todosArchive/);
  assert.match(todos, /result:\s*\{\s*kind:\s*'fact'/);
  assert.match(todos, /reconcile:\s*\{\s*kind:\s*'refetch'/);
  assert.match(todos, /optimistic:/);
  assert.match(todos, /QueryCache|gql\.cache|seedQueryCache/);
  assert.doesNotMatch(todos, /from '\$lib\/api\/commands\.generated'/);
  assert.doesNotMatch(todos, /url:\s*['"]\/graphql['"]/);
  // No hand-rolled pending map / mergeFromServer — pipeline owns latency compensation
  assert.doesNotMatch(todos, /function mergeFromServer/);
  assert.doesNotMatch(todos, /let pending = \$state/);

  const chat = fs.readFileSync(path.join(uiRoot, 'src/routes/chat/+page.svelte'), 'utf8');
  assert.match(chat, /gql\.commands\.chatMessagesPost/);
  assert.match(chat, /optimistic:/);
  assert.match(chat, /result:\s*\{\s*kind:\s*'fact'/);
  assert.match(chat, /reconcile:\s*\{\s*kind:\s*'subscription'/);
  assert.doesNotMatch(chat, /from '\$lib\/api\/commands\.generated'/);

  const admin = fs.readFileSync(path.join(uiRoot, 'src/routes/admin/+page.svelte'), 'utf8');
  assert.match(admin, /gql\.commands\.todosForceArchive/);
  assert.match(admin, /optimistic:/);
  assert.match(admin, /reconcile:\s*\{\s*kind:\s*'refetch'/);
  assert.doesNotMatch(admin, /from '\$lib\/api\/commands\.generated'/);
});

test('generated mutations are multiline; operations.gql is copy-paste ready', () => {
  const gen = fs.readFileSync(generatedPath, 'utf8');
  const gql = fs.readFileSync(operationsGql, 'utf8');
  assert.match(gen, /COMMAND_DOCS = \{/);
  assert.match(gen, /"todos_create": `\nmutation Command_todos_create/);
  assert.match(gql, /mutation Command_todos_create\(\$input: TodoCreateInput!\) \{/);
  assert.match(gql, /todos_create\(input: \$input\) \{\n/);
});

test('httpUrlToWsUrl maps HTTP GraphQL paths to /graphql/ws', async () => {
  const wsFile = path.join(uiRoot, 'src/lib/graphql-ws.ts');
  const { httpUrlToWsUrl } = await import(pathToFileURL(wsFile).href);
  const rel = httpUrlToWsUrl('/graphql');
  assert.match(rel, /\/graphql\/ws$/);
  assert.match(rel, /^ws:/);
  assert.equal(httpUrlToWsUrl('http://127.0.0.1:8791/graphql'), 'ws://127.0.0.1:8791/graphql/ws');
  assert.equal(httpUrlToWsUrl('https://api.example/graphql'), 'wss://api.example/graphql/ws');
});

test('chat page uses gql.subscribe (bound client), not raw authFromPageData', () => {
  const chat = fs.readFileSync(path.join(uiRoot, 'src/routes/chat/+page.svelte'), 'utf8');
  assert.match(chat, /gql\.subscribe\s*\(/);
  assert.doesNotMatch(chat, /authFromPageData/);
  assert.doesNotMatch(chat, /from '\$lib\/graphql-ws'/);
});
