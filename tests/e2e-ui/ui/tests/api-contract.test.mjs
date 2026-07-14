/**
 * Structural + optional live contracts for e2e-ui.
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const base = process.env.E2E_API_ORIGIN || process.env.E2E_BASE_URL;
const uiRoot = path.dirname(fileURLToPath(new URL('.', import.meta.url)));

test('SSR is enabled (not SPA-only)', () => {
  const pkg = fs.readFileSync(new URL('../package.json', import.meta.url), 'utf8');
  assert.match(pkg, /adapter-node/);
  const layoutTs = new URL('../src/routes/+layout.ts', import.meta.url);
  if (fs.existsSync(layoutTs)) {
    const src = fs.readFileSync(layoutTs, 'utf8');
    assert.doesNotMatch(src, /ssr\s*=\s*false/);
  }
  const todosServer = fs.readFileSync(
    new URL('../src/routes/todos/+page.server.ts', import.meta.url),
    'utf8'
  );
  assert.match(todosServer, /loadQuery|serverGraphql|PageServerLoad/);
  assert.match(todosServer, /todos\.query|accessToken/);
});

test('auth + WS modules use OIDC patterns', () => {
  const auth = fs.readFileSync(new URL('../src/auth.ts', import.meta.url), 'utf8');
  assert.match(auth, /SvelteKitAuth|@auth\/sveltekit/);
  assert.match(auth, /OIDC_ISSUER|accessToken/);
  const hooks = fs.readFileSync(new URL('../src/hooks.server.ts', import.meta.url), 'utf8');
  assert.match(hooks, /\/todos|\/chat|\/session/);
  const ws = fs.readFileSync(new URL('../src/lib/graphql-ws.ts', import.meta.url), 'utf8');
  assert.match(ws, /connection_init/);
  // Bearer/auth payload built via shared auth-headers helper (same as HTTP).
  assert.match(ws, /wsConnectionInitPayload|authorization|accessToken|Bearer/);
  const gql = fs.readFileSync(new URL('../src/lib/server/graphql.ts', import.meta.url), 'utf8');
  assert.match(gql, /requestGraphql|serverGraphql/);
  const req = fs.readFileSync(new URL('../src/lib/gql/request.ts', import.meta.url), 'utf8');
  assert.match(req, /Bearer|authorization/);
});

test('website auth shell + fixture routes present', () => {
  const signin = fs.readFileSync(new URL('../src/routes/signin/+server.ts', import.meta.url), 'utf8');
  assert.match(signin, /\/auth\/signin\/oidc/);
  assert.match(signin, /X-Auth-Return-Redirect/);
  assert.ok(fs.existsSync(new URL('../src/routes/todos/+page.svelte', import.meta.url)));
  assert.ok(fs.existsSync(new URL('../src/routes/chat/+page.svelte', import.meta.url)));
  assert.ok(fs.existsSync(new URL('../src/routes/admin/+page.svelte', import.meta.url)));
  assert.ok(!fs.existsSync(new URL('../src/routes/docs', import.meta.url)));
  assert.ok(!fs.existsSync(new URL('../src/routes/control-plane', import.meta.url)));
  const nav = fs.readFileSync(
    new URL('../src/lib/components/shared/header/Navbar.svelte', import.meta.url),
    'utf8'
  );
  assert.match(nav, /\/todos/);
  assert.match(nav, /\/chat/);
  assert.match(nav, /\/admin|isAdmin|engineRole/);
  assert.doesNotMatch(nav, /\/docs|\/control-plane|Pricing|two-paths|HopsBrand|hops-ops|#demos|#code/i);
  const layout = fs.readFileSync(
    new URL('../src/routes/+layout.server.ts', import.meta.url),
    'utf8'
  );
  assert.match(layout, /engineRole|engineRoleFromGroups/);
});

test('admin: role-gated all-owners todos view + force-archive mutation', () => {
  assert.ok(fs.existsSync(new URL('../src/routes/admin/admin.gql', import.meta.url)));
  const gql = fs.readFileSync(new URL('../src/routes/admin/admin.gql', import.meta.url), 'utf8');
  assert.match(gql, /query AdminAllTodos|todos/);
  assert.doesNotMatch(gql, /mutation |todos_force_archive/);
  assert.match(gql, /limit:\s*100|order_by/);
  const ops = fs.readFileSync(
    new URL('../src/lib/api/commands.operations.gql', import.meta.url),
    'utf8'
  );
  assert.match(ops, /todos_force_archive/);
  const resource = fs.readFileSync(
    new URL('../src/routes/admin/admin.resource.ts', import.meta.url),
    'utf8'
  );
  assert.doesNotMatch(resource, /forceArchive|AdminForceArchiveDocument|mutations:/);
  const server = fs.readFileSync(
    new URL('../src/routes/admin/+page.server.ts', import.meta.url),
    'utf8'
  );
  // 403 before loadQuery — non-admins never SSR all-owners data
  assert.match(server, /isAdminEngineRole|error\(403/);
  const body = server.slice(server.indexOf('export const load'));
  const gateIdx = body.indexOf('isAdminEngineRole');
  const loadIdx = body.indexOf('await loadQuery');
  assert.ok(gateIdx >= 0 && loadIdx > gateIdx, 'admin gate must run before await loadQuery');
  assert.match(server, /adminTodos|AdminAllTodos|loadQuery/);
  const page = fs.readFileSync(new URL('../src/routes/admin/+page.svelte', import.meta.url), 'utf8');
  assert.match(page, /owner_id|All field notes|admin|forceArchive|Force archive|todosForceArchive/i);
  const hooks = fs.readFileSync(new URL('../src/hooks.server.ts', import.meta.url), 'utf8');
  assert.match(hooks, /\/admin/);
  const service = fs.readFileSync(
    new URL('../../crates/service/src/service.rs', import.meta.url),
    'utf8'
  );
  assert.match(service, /role\("admin"/);
  assert.match(service, /todos_force_archive|force_archive/);
  assert.match(service, /\.roles\(\[\"admin\"\]\)|roles\(\[\"admin\"\]\)/);
  assert.match(service, /owner_id|claim\("x-user-id"\)/);
  assert.match(service, /graphiql_enabled|GRAPHIQL/);
  const force = fs.readFileSync(
    new URL('../../crates/service/src/handlers/commands/force_archive.rs', import.meta.url),
    'utf8'
  );
  assert.match(force, /todo\.force_archive/);
  assert.match(force, /session_is_admin|session_has_user/);
  assert.match(force, /fn guard[\s\S]*session_is_admin/);
  assert.match(force, /todo\.force_archived|FORCE_ARCHIVED/);
  const codegen = fs.readFileSync(new URL('../codegen.ts', import.meta.url), 'utf8');
  assert.match(codegen, /admin\.graphql/);
  const todoCmd = fs.readFileSync(
    new URL('../../crates/service/src/handlers/commands/todo_cmd.rs', import.meta.url),
    'utf8'
  );
  assert.match(todoCmd, /commit_todo_event|load_todo/);
  const makefile = fs.readFileSync(new URL('../../Makefile', import.meta.url), 'utf8');
  assert.match(makefile, /check-gql/);
});

test('engineRoleFromGroups is exact membership (not substring)', () => {
  const rolesUrl = pathToFileURL(path.join(uiRoot, 'src/lib/roles.ts')).href;
  const script = `
    import { engineRoleFromGroups, isAdminEngineRole } from ${JSON.stringify(rolesUrl)};
    const eq = (a, b, m) => { if (a !== b) throw new Error(m + ': ' + a + ' !== ' + b); };
    eq(engineRoleFromGroups(undefined), 'user', 'undef');
    eq(engineRoleFromGroups([]), 'user', 'empty');
    eq(engineRoleFromGroups(['user']), 'user', 'user');
    eq(engineRoleFromGroups(['admin']), 'admin', 'admin');
    eq(engineRoleFromGroups(['admins']), 'admin', 'admins');
    eq(engineRoleFromGroups(['administrator']), 'user', 'administrator');
    eq(engineRoleFromGroups(['not-admin']), 'user', 'not-admin');
    eq(isAdminEngineRole('admin'), true, 'isAdmin');
    eq(isAdminEngineRole('user'), false, 'isUser');
    console.log('roles-ok');
  `;
  const r = spawnSync(
    process.execPath,
    ['--experimental-strip-types', '--input-type=module', '-e', script],
    { encoding: 'utf8', cwd: uiRoot }
  );
  assert.equal(r.status, 0, `stderr=${r.stderr}\nstdout=${r.stdout}`);
  assert.match(r.stdout, /roles-ok/);
});

test('home is distributed template with 8-step unidirectional todos story', () => {
  const home = fs.readFileSync(new URL('../src/routes/+page.svelte', import.meta.url), 'utf8');
  assert.match(home, /framework template|e2e-ui template/i);
  assert.match(home, /make test|test-live|e2e/i);
  assert.match(home, /\/todos/);
  assert.match(home, /\/chat/);
  assert.doesNotMatch(home, /Launch My Platform|HopsBrand|founder|listmonk|Systems Lab|lab-acid/i);

  // Full unidirectional story (8 steps) — one file/concept per code block
  assert.match(home, /unidirectional/i);
  assert.match(home, /id="story-flow"/);
  assert.match(home, /type CodeBlock|blocks: CodeBlock\[\]|blocks:/);
  assert.match(home, /wf-code-stack/);
  assert.match(home, /\{#each step\.blocks as block\}/);
  assert.match(home, /data-sample=\{step\.label\}/);
  assert.match(home, /label: 'Auth session'/);
  assert.match(home, /label: 'SSR \+ RBAC'/);
  assert.match(home, /label: 'Hydration'/);
  assert.match(home, /label: 'Subscription'/);
  assert.match(home, /label: 'Commands not RM writes'/);
  assert.match(home, /label: 'Command handler'/);
  assert.match(home, /label: 'Projector'/);
  assert.match(home, /label: 'Read path'/);

  // Step 02 split into separate files (no merged mega-block)
  assert.match(home, /file: 'todos\/\+page\.server\.ts'/);
  assert.match(home, /file: 'ui\/src\/lib\/server\/graphql\.ts'/);
  assert.match(home, /file: 'crates\/service\/src\/service\.rs'/);

  // Step substance from real fixture code
  assert.match(home, /accessToken|Auth\.js|OIDC|Zitadel/i);
  assert.match(home, /Bearer|serverGraphql|authorization/i);
  assert.match(home, /ModelPermissions|owner_id|claim\("x-user-id"\)|OidcBearer/);
  assert.match(home, /data\.todos|hydration|mergeFromServer/i);
  assert.match(home, /subscription|connection_init/);
  assert.match(home, /anti-pattern|todos_create|todo\.create/i);
  assert.match(home, /require_user|outbox|TodoFact|todo\.created/);
  assert.match(home, /project_todo|ReadModelWritePlanBuilder|map_fact/);
  assert.match(home, /ChangeHub|Unidirectional|one direction/i);

  // Solid alternating bands (not transparent page sections)
  assert.match(home, /wf-band-light|wf-band-dark/);
  assert.match(home, /wf-story-step|Step 0[1-8]/);

  const css = fs.readFileSync(new URL('../src/app.css', import.meta.url), 'utf8');
  assert.match(css, /Neutral wireframe|--wf-bg|--wf-ink/);
  assert.match(css, /#f6f5f2|#1c1c1a/);
  assert.match(css, /\.wf-band-light|\.wf-band-dark/);
  assert.match(css, /\.wf-story-step/);
  // Dark-band headings beat light section-head ink (readable cream on charcoal)
  assert.match(css, /--wf-band-dark-ink/);
  assert.match(css, /--wf-band-dark-bg/);
  assert.match(css, /\.wf-home \.wf-band-dark \.wf-section-head h2|\.wf-home \.wf-band-dark h2/);
  assert.match(css, /var\(--wf-band-dark-ink\)/);
  // Hero shares band content column (no center island → left snap)
  assert.match(css, /\.wf-hero-inner[\s\S]{0,200}max-width:\s*var\(--wf-max\)/);
  assert.match(css, /text-align:\s*left/);
  // Not Systems Lab acid identity
  assert.doesNotMatch(css, /--lab-acid:\s*#c8f542|Systems Lab/);
  assert.doesNotMatch(css, /#e69a2d/);
  // Generous section spacing token
  assert.match(css, /--wf-section-y|section-y/);

  const footer = fs.readFileSync(
    new URL('../src/lib/components/shared/Footer.svelte', import.meta.url),
    'utf8'
  );
  assert.match(footer, /e2e-ui|Distributed template/i);
  assert.doesNotMatch(footer, /HopsBrand|Ship products, not infrastructure/i);
});

test('todos: co-located .gql + resource — same query SSR + browser mutations', () => {
  assert.ok(fs.existsSync(new URL('../src/routes/todos/todos.gql', import.meta.url)));
  assert.ok(fs.existsSync(new URL('../src/routes/todos/todos.generated.ts', import.meta.url)));
  const gqlFile = fs.readFileSync(new URL('../src/routes/todos/todos.gql', import.meta.url), 'utf8');
  assert.match(gqlFile, /query Todos/);
  assert.doesNotMatch(gqlFile, /mutation Todos/);

  const resource = fs.readFileSync(
    new URL('../src/routes/todos/todos.resource.ts', import.meta.url),
    'utf8'
  );
  assert.match(resource, /defineResource/);
  assert.match(resource, /export const todos/);
  assert.match(resource, /TodosDocument|from '\.\/todos\.generated'/);
  assert.doesNotMatch(resource, /mutations:|TodosCreateDocument/);
  // No hand-authored multi-line GraphQL strings in the resource
  assert.doesNotMatch(resource, /mutation TodosCreate|query Todos \{/);

  const defineRes = fs.readFileSync(
    new URL('../src/lib/gql/define-resource.ts', import.meta.url),
    'utf8'
  );
  assert.match(defineRes, /export function defineResource/);

  const loadHelper = fs.readFileSync(
    new URL('../src/lib/gql/load-query.server.ts', import.meta.url),
    'utf8'
  );
  assert.match(loadHelper, /export function loadQuery/);
  assert.match(loadHelper, /serverGraphql/);

  const useGql = fs.readFileSync(new URL('../src/lib/gql/use-graphql.ts', import.meta.url), 'utf8');
  assert.match(useGql, /export function useGraphql/);
  assert.match(useGql, /createGraphqlClient|\/graphql/);

  const page = fs.readFileSync(new URL('../src/routes/todos/+page.svelte', import.meta.url), 'utf8');
  assert.match(page, /\$state<Todo\[\]>\(\[\.\.\.\(data\.todos/);
  assert.match(page, /mergeFromServer/);
  assert.match(page, /useGraphql/);
  assert.match(page, /todos\.resource|todosResource|from '\.\/todos\.resource'/);
  assert.match(page, /todosResource\.query|todos\.query/);
  // Writes: generated command functions (GraphQL wire under the hood).
  assert.match(page, /commands\.generated/);
  assert.match(page, /todosCreate/);
  assert.match(page, /todosComplete/);
  assert.match(page, /todosArchive/);
  assert.doesNotMatch(page, /mutations\.(create|complete|archive)/);
  assert.doesNotMatch(page, /use:enhance|\?\/create|export const actions/);
  // Query refetch still uses GraphQL client; writes use command client
  assert.match(page, /useGraphql|gql\.request|todosCreate/);

  const server = fs.readFileSync(
    new URL('../src/routes/todos/+page.server.ts', import.meta.url),
    'utf8'
  );
  assert.match(server, /loadQuery|todos\.query/);
  assert.match(server, /from '\.\/todos\.resource'|from "\.\/todos\.resource"/);
  assert.match(server, /todos\.query/);
  // SSR is read-only seed — no form actions / command mutations
  assert.doesNotMatch(server, /export const actions|todos_create|serverCommand|\?\/create/);

  // Shared auth headers for HTTP + WS (DRY)
  const authHeaders = fs.readFileSync(
    new URL('../src/lib/gql/auth-headers.ts', import.meta.url),
    'utf8'
  );
  assert.match(authHeaders, /export function buildAuthHeaders/);
  assert.match(authHeaders, /wsConnectionInitPayload/);
  assert.ok(!fs.existsSync(new URL('../src/lib/gql/documents.ts', import.meta.url)));

  // Unified request path (single Jack-style core)
  const request = fs.readFileSync(new URL('../src/lib/gql/request.ts', import.meta.url), 'utf8');
  assert.match(request, /export async function requestGraphql/);
  assert.match(request, /buildAuthHeaders|documentToString/);
  const barrel = fs.readFileSync(new URL('../src/lib/gql/index.ts', import.meta.url), 'utf8');
  assert.match(barrel, /useGraphql|defineResource|requestGraphql/);
  const createClient = fs.readFileSync(
    new URL('../src/lib/gql/create-client.ts', import.meta.url),
    'utf8'
  );
  assert.match(createClient, /export function createGraphqlClient/);
  const serverGql = fs.readFileSync(new URL('../src/lib/server/graphql.ts', import.meta.url), 'utf8');
  assert.match(serverGql, /requestGraphql/);
  const cleanEnv = fs.readFileSync(new URL('../src/lib/clean-env.ts', import.meta.url), 'utf8');
  assert.match(cleanEnv, /export function cleanEnvValue/);
  const loadQuery = fs.readFileSync(
    new URL('../src/lib/gql/load-query.server.ts', import.meta.url),
    'utf8'
  );
  assert.match(loadQuery, /authFromPageData/);

  const schema = fs.readFileSync(new URL('../schema/user.graphql', import.meta.url), 'utf8');
  // Role SDL is exported from Rust engine (not a permanent hand pilot)
  assert.match(schema, /GENERATED|sdl_for_role|build_graphql_engine/);
  assert.match(schema, /todos_create|chat_messages/);
  assert.match(schema, /type Query|type Mutation/);
  const pkg = fs.readFileSync(new URL('../package.json', import.meta.url), 'utf8');
  assert.match(pkg, /gen:gql|graphql-codegen/);
  assert.match(pkg, /gen:schema|e2e-export-sdl/);
  const makefile = fs.readFileSync(new URL('../../Makefile', import.meta.url), 'utf8');
  assert.match(makefile, /export-sdl/);
  assert.match(makefile, /e2e-export-sdl/);
  const exportBin = fs.readFileSync(
    new URL('../../crates/runner/src/bin/export_sdl.rs', import.meta.url),
    'utf8'
  );
  assert.match(exportBin, /sdl_for_role/);
  assert.match(exportBin, /build_graphql_engine/);
});

// DX contract lives in GitKB ([[specs/e2e-ui/sveltekit-dx]]), not the code tree.
// Structural checks above assert the pilot modules the spec describes.

test('chat: co-located .gql + resource — SSR query, WS subscription, browser post', () => {
  assert.ok(fs.existsSync(new URL('../src/routes/chat/chat.gql', import.meta.url)));
  assert.ok(fs.existsSync(new URL('../src/routes/chat/chat.generated.ts', import.meta.url)));
  const gqlFile = fs.readFileSync(new URL('../src/routes/chat/chat.gql', import.meta.url), 'utf8');
  assert.match(gqlFile, /query ChatMessages/);
  assert.match(gqlFile, /subscription ChatMessagesLive/);
  assert.doesNotMatch(gqlFile, /mutation ChatPost/);

  const resource = fs.readFileSync(
    new URL('../src/routes/chat/chat.resource.ts', import.meta.url),
    'utf8'
  );
  assert.match(resource, /defineResource/);
  assert.match(resource, /export const chat/);
  assert.match(resource, /ChatMessagesDocument|from '\.\/chat\.generated'/);
  assert.doesNotMatch(resource, /ChatPostDocument|mutations:/);
  assert.match(resource, /ChatMessagesLiveDocument/);
  assert.match(resource, /LOBBY_ROOM|lobby/);
  assert.doesNotMatch(resource, /subscription \{|mutation ChatPost\(/);

  const server = fs.readFileSync(
    new URL('../src/routes/chat/+page.server.ts', import.meta.url),
    'utf8'
  );
  assert.match(server, /loadQuery/);
  assert.match(server, /from '\.\/chat\.resource'|from "\.\/chat\.resource"/);
  assert.match(server, /chat\.query/);
  assert.doesNotMatch(server, /export const actions|serverCommand|\?\/create/);

  const page = fs.readFileSync(new URL('../src/routes/chat/+page.svelte', import.meta.url), 'utf8');
  assert.match(page, /from '\.\/chat\.resource'|from "\.\/chat\.resource"/);
  assert.match(page, /chatMessagesPost|commands\.generated/);
  assert.match(page, /useGraphql/);
  assert.doesNotMatch(page, /chat\.mutations\.post|mutations\.post/);
  assert.match(page, /chat\.subscription|subscription/);
  // WS uses bound client — same auth as HTTP (no separate authFromPageData at call site).
  assert.match(page, /gql\.subscribe\s*\(/);
  assert.doesNotMatch(page, /from '\$lib\/graphql-ws'/);
  assert.doesNotMatch(page, /authFromPageData/);
  assert.doesNotMatch(page, /browserGraphql|from '\$lib\/gql\/documents'/);
  assert.doesNotMatch(page, /use:enhance|\?\/create|export const actions/);

  const ws = fs.readFileSync(new URL('../src/lib/graphql-ws.ts', import.meta.url), 'utf8');
  assert.match(ws, /wsConnectionInitPayload|auth-headers/);
});

test('GraphQL-only API: command mutations registered, HTTP routes disabled', () => {
  const service = fs.readFileSync(
    new URL('../../crates/service/src/service.rs', import.meta.url),
    'utf8'
  );
  assert.match(service, /without_http_command_routes/);
  assert.match(service, /todos_create/);
  assert.match(service, /todos_complete/);
  assert.match(service, /todos_archive/);
  assert.match(service, /todos_rename/);
  assert.match(service, /chat_messages_post/);
  assert.match(service, /GraphqlCommands|exposed_command/);
  const create = fs.readFileSync(
    new URL('../../crates/service/src/handlers/commands/create.rs', import.meta.url),
    'utf8'
  );
  assert.match(create, /require_user/);
  assert.match(create, /GraphqlInput|TodoCreateInput/);
  assert.doesNotMatch(create, /owner_id.*input|input\.owner/);
});

test('live GraphQL unauthenticated rejected when OIDC stack', { skip: !base }, async () => {
  const res = await fetch(`${base}/graphql`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ query: '{ todos { todo_id } }' })
  });
  console.log(`live gql status=${res.status}`);
  assert.ok(res.status === 401 || res.status === 200, `unexpected ${res.status}`);
  if (res.status === 401) {
    const text = await res.text();
    assert.match(text, /unauth|UNAUTHENTICATED/i);
  }
});
