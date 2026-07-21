/**
 * Command client: generator + generated module drive the real client.request path.
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { spawnSync } from 'node:child_process';
import { httpUrlToWsUrl } from '@hops-ops/distributed';
import {
  buildMutationOperation,
  fieldToFunctionName,
  generateCommandPoliciesTs,
  generateCommandsTs,
  generateOperationsGql
} from '@hops-ops/distributed/codegen';
import {
  bindCommands as bindDistributedCommands,
  defineCommand,
  defineCommands
} from '@hops-ops/distributed/commands';

const here = path.dirname(fileURLToPath(import.meta.url));
const uiRoot = path.resolve(here, '..');
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

test('generic command entry point binds app-owned definitions, including zero-input commands', async () => {
  const definitions = defineCommands({
    ping: defineCommand({
      field: 'ping',
      document: 'mutation Ping { ping }',
      hasInput: false
    })
  });
  let seenVariables = 'not-called';
  const commands = bindDistributedCommands(
    {
      async request(_document, variables) {
        seenVariables = variables;
        return { data: { ping: { ok: true } }, status: 200 };
      }
    },
    definitions
  );

  const result = await commands.ping();
  assert.deepEqual(result.data, { ok: true });
  assert.equal(result.status, 200);
  assert.equal(seenVariables, undefined);
});

test('published codegen creates aligned operations, commands, and policies', () => {
  assert.equal(fieldToFunctionName('todos_create'), 'todosCreate');
  const catalog = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
  const gql = generateOperationsGql(catalog);
  const ts = generateCommandsTs(catalog);
  assert.match(gql, /mutation Command_todos_create/);
  assert.match(gql, /todos_force_archive/);
  assert.match(ts, /export function todosCreate/);
  assert.match(ts, /COMMAND_DOCS/);
  assert.match(ts, /COMMANDS = defineCommands/);
  assert.match(ts, /CommandClient/);
  // Same op body in both artifacts
  const text = buildMutationOperation(catalog.commands[0]);
  assert.ok(gql.includes(text));
  assert.ok(ts.includes(text.split('\n')[0]));

  const policies = generateCommandPoliciesTs(catalog);
  assert.match(policies, /todosCreate/);
  assert.match(policies, /blobGamesStart/);
  assert.match(policies, /"projection"/);
  assert.match(policies, /satisfies CommandPolicyMap<typeof COMMANDS>/);
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
    if (mod.COMMANDS.todosCreate.document !== seen.document) throw new Error('COMMANDS mismatch');
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

test('app pages use gql.store/live + commands pipeline (cache transparent)', () => {
  const todos = fs.readFileSync(pageSvelte, 'utf8');
  assert.match(todos, /gql\.store\s*\(/);
  assert.match(todos, /\$list\.data/);
  assert.match(todos, /gql\.commands\.todosCreate/);
  assert.match(todos, /gql\.commands\.todosComplete/);
  assert.match(todos, /gql\.commands\.todosArchive/);
  assert.match(todos, /list:\s*\{\s*at:\s*'todos',\s*by:\s*'todo_id'/);
  assert.match(todos, /list\.scheduleCatchUp|scheduleCatchUp/);
  assert.match(todos, /list\.target\(/);
  assert.doesNotMatch(todos, /seedQueryCache|readQueryList|syncFromCache/);
  assert.doesNotMatch(todos, /from '\$lib\/api\/commands\.generated'/);
  assert.doesNotMatch(todos, /function mergeFromServer/);

  // Policies come from Rust client_reconcile → manifest → generated TS.
  const policies = fs.readFileSync(
    path.join(uiRoot, 'src/lib/api/commands.policies.generated.ts'),
    'utf8'
  );
  assert.match(policies, /commandPolicies/);
  assert.match(policies, /todosCreate/);
  assert.match(policies, /kind:\s*"fact"/);
  assert.match(policies, /kind:\s*"none"/);
  assert.match(policies, /blobGamesMove/);
  assert.match(policies, /kind:\s*"projection"/);
  assert.match(policies, /chatMessagesPost/);
  assert.match(policies, /kind:\s*"subscription"/);

  const chat = fs.readFileSync(path.join(uiRoot, 'src/routes/chat/+page.svelte'), 'utf8');
  assert.match(chat, /gql\.live\s*\(/);
  assert.match(chat, /\$lobby\.(data|status)/);
  assert.match(chat, /gql\.commands\.chatMessagesPost/);
  assert.match(chat, /lobby\.target\(/);
  assert.match(chat, /list:\s*\{\s*at:\s*'chat_messages'/);
  assert.doesNotMatch(chat, /seedQueryCache|gql\.subscribe\s*\(/);
  assert.doesNotMatch(chat, /from '\$lib\/api\/commands\.generated'/);

  const admin = fs.readFileSync(path.join(uiRoot, 'src/routes/admin/+page.svelte'), 'utf8');
  assert.match(admin, /gql\.store\s*\(/);
  assert.match(admin, /gql\.commands\.todosForceArchive/);
  assert.match(admin, /list\.target\(/);
  assert.match(admin, /scheduleCatchUp/);
  assert.doesNotMatch(admin, /seedQueryCache|readQueryList/);
});

test('generated mutations are multiline; operations.gql is copy-paste ready', () => {
  const gen = fs.readFileSync(generatedPath, 'utf8');
  const gql = fs.readFileSync(operationsGql, 'utf8');
  assert.match(gen, /COMMAND_DOCS = \{/);
  assert.match(gen, /"todos_create": `\nmutation Command_todos_create/);
  assert.match(gql, /mutation Command_todos_create\(\$input: TodoCreateInput!\) \{/);
  assert.match(gql, /todos_create\(input: \$input\) \{\n/);
});

test('public client maps HTTP GraphQL paths to /graphql/ws', () => {
  const rel = httpUrlToWsUrl('/graphql');
  assert.match(rel, /\/graphql\/ws$/);
  assert.match(rel, /^ws:/);
  assert.equal(httpUrlToWsUrl('http://127.0.0.1:8791/graphql'), 'ws://127.0.0.1:8791/graphql/ws');
  assert.equal(httpUrlToWsUrl('https://api.example/graphql'), 'wss://api.example/graphql/ws');
});

test('chat page uses gql.live (not raw subscribe / authFromPageData)', () => {
  const chat = fs.readFileSync(path.join(uiRoot, 'src/routes/chat/+page.svelte'), 'utf8');
  assert.match(chat, /gql\.live\s*\(/);
  assert.doesNotMatch(chat, /gql\.subscribe\s*\(/);
  assert.doesNotMatch(chat, /authFromPageData/);
  assert.doesNotMatch(chat, /from '\$lib\/graphql-ws'/);
});
