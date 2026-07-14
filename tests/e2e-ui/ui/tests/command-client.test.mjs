/**
 * Command client: generator + generated module drive the real requestGraphql path.
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
  assert.equal(byField.todos_create.input.fields.some((f) => f.name === 'title'), true);
});

test('generateCommandsTs produces todosCreate/todosComplete from real manifest', async () => {
  const { generateCommandsTs, fieldToFnName } = await import(
    pathToFileURL(genScript).href
  );
  assert.equal(fieldToFnName('todos_create'), 'todosCreate');
  const catalog = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
  const ts = generateCommandsTs(catalog);
  assert.match(ts, /export async function todosCreate/);
  assert.match(ts, /export async function todosComplete/);
  assert.match(ts, /requestGraphql/);
  assert.match(ts, /todos_force_archive/);
  assert.match(ts, /COMMAND_ROLES/);
});

test('generated todosCreate posts GraphQL mutation via real requestGraphql', async () => {
  const requestFile = path.join(uiRoot, 'src/lib/gql/request.ts');
  const genFile = generatedPath;
  assert.ok(fs.existsSync(genFile), 'commands.generated.ts missing — run make gen-commands');

  const script = `
    import { pathToFileURL } from 'node:url';
    // Load generator-built module: rewrite relative imports for node strip-types.
    // Instead import requestGraphql and call the same contract the generator emits.
    import { requestGraphql } from ${JSON.stringify(pathToFileURL(requestFile).href)};

    let seen;
    globalThis.fetch = async (url, init) => {
      seen = { url, body: JSON.parse(init.body), headers: init.headers };
      return {
        status: 200,
        json: async () => ({
          data: {
            todos_create: {
              todo_id: 't1',
              owner_id: 'alice',
              title: 'hi',
              status: 'open'
            }
          }
        })
      };
    };

    const document =
      'mutation Command_todos_create($input: TodoCreateInput!) { todos_create(input: $input) { todo_id owner_id title status } }';
    const result = await requestGraphql(
      '/graphql',
      document,
      { accessToken: 'tok' },
      { input: { todo_id: 't1', title: 'hi' } }
    );
    if (!result.data?.todos_create) throw new Error('missing data');
    if (seen.url !== '/graphql') throw new Error('bad url');
    if (!seen.body.query.includes('todos_create')) throw new Error('mutation missing');
    if (seen.body.variables.input.title !== 'hi') throw new Error('vars missing');
    if (!String(seen.headers.Authorization || seen.headers.authorization).includes('tok')) {
      throw new Error('auth missing');
    }
    // Also load generated module and call todosCreate (shipped entry).
    const mod = await import(${JSON.stringify(pathToFileURL(genFile).href)});
    const r2 = await mod.todosCreate(
      { todo_id: 't2', title: 'x' },
      { url: '/graphql', auth: { accessToken: 'tok' } }
    );
    if (!r2.data || r2.data.todo_id !== 't1') throw new Error('todosCreate unwrap failed');
    if (typeof mod.todosComplete !== 'function') throw new Error('todosComplete missing');
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

test('app pages use generated command functions for all writes', () => {
  const todos = fs.readFileSync(pageSvelte, 'utf8');
  assert.match(todos, /from '\$lib\/api\/commands\.generated'/);
  assert.match(todos, /todosCreate\s*\(/);
  assert.match(todos, /todosComplete\s*\(/);
  assert.match(todos, /todosArchive\s*\(/);
  assert.doesNotMatch(todos, /todosResource\.mutations\./);
  assert.doesNotMatch(todos, /mutation TodosCreate/);

  const chat = fs.readFileSync(path.join(uiRoot, 'src/routes/chat/+page.svelte'), 'utf8');
  assert.match(chat, /chatMessagesPost\s*\(/);
  assert.doesNotMatch(chat, /chat\.mutations\.post|mutations\.post/);

  const admin = fs.readFileSync(path.join(uiRoot, 'src/routes/admin/+page.svelte'), 'utf8');
  assert.match(admin, /todosForceArchive\s*\(/);
  assert.doesNotMatch(admin, /mutations\.forceArchive|adminTodos\.mutations/);
});

test('generated mutations are multiline template strings', () => {
  const gen = fs.readFileSync(generatedPath, 'utf8');
  assert.match(gen, /const document = `\nmutation Command_todos_create/);
  assert.match(gen, /todos_create\(input: \$input\) \{\n/);
  assert.match(gen, /todo_id\n/);
});
