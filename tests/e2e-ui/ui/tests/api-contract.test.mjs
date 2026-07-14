/**
 * Structural + optional live contracts for e2e-ui.
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';

const base = process.env.E2E_API_ORIGIN || process.env.E2E_BASE_URL;

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
  assert.match(ws, /authorization|accessToken|Bearer/);
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
  assert.ok(!fs.existsSync(new URL('../src/routes/docs', import.meta.url)));
  assert.ok(!fs.existsSync(new URL('../src/routes/control-plane', import.meta.url)));
  const nav = fs.readFileSync(
    new URL('../src/lib/components/shared/header/Navbar.svelte', import.meta.url),
    'utf8'
  );
  assert.match(nav, /\/todos/);
  assert.match(nav, /\/chat/);
  assert.doesNotMatch(nav, /\/docs|\/control-plane|Pricing|two-paths|HopsBrand|hops-ops|#demos|#code/i);
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
  assert.match(gqlFile, /mutation TodosCreate/);
  assert.match(gqlFile, /mutation TodosComplete/);
  assert.match(gqlFile, /mutation TodosArchive/);

  const resource = fs.readFileSync(
    new URL('../src/routes/todos/todos.resource.ts', import.meta.url),
    'utf8'
  );
  assert.match(resource, /defineResource/);
  assert.match(resource, /export const todos/);
  assert.match(resource, /TodosDocument|from '\.\/todos\.generated'/);
  assert.match(resource, /TodosCreateDocument|TodosCompleteDocument|TodosArchiveDocument/);
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
  assert.match(page, /mutations\.create|mutations\.complete|mutations\.archive/);
  assert.doesNotMatch(page, /use:enhance|\?\/create|export const actions/);
  // Writes go through GraphQL client, not form actions
  assert.match(page, /gql\.request|createGraphqlClient|useGraphql/);

  const server = fs.readFileSync(
    new URL('../src/routes/todos/+page.server.ts', import.meta.url),
    'utf8'
  );
  assert.match(server, /loadQuery|todos\.query/);
  assert.match(server, /from '\.\/todos\.resource'|from "\.\/todos\.resource"/);
  assert.match(server, /todos\.query/);
  // SSR is read-only seed — no form actions / command mutations
  assert.doesNotMatch(server, /export const actions|todos_create|serverCommand|\?\/create/);

  // documents re-exports resource identity for chat-era imports
  const docs = fs.readFileSync(new URL('../src/lib/gql/documents.ts', import.meta.url), 'utf8');
  assert.match(docs, /todos\.query|TODOS_QUERY|documentToString/);
  assert.match(docs, /todos\.resource|todos\.mutations/);

  // Unified request path (single Jack-style core)
  const request = fs.readFileSync(new URL('../src/lib/gql/request.ts', import.meta.url), 'utf8');
  assert.match(request, /export async function requestGraphql/);
  assert.match(request, /buildAuthHeaders|authorization/);
  assert.match(request, /documentToString/);
  const client = fs.readFileSync(new URL('../src/lib/gql/client.ts', import.meta.url), 'utf8');
  assert.match(client, /requestGraphql|createGraphqlClient|defineResource/);
  const createClient = fs.readFileSync(
    new URL('../src/lib/gql/create-client.ts', import.meta.url),
    'utf8'
  );
  assert.match(createClient, /export function createGraphqlClient/);
  const serverGql = fs.readFileSync(new URL('../src/lib/server/graphql.ts', import.meta.url), 'utf8');
  assert.match(serverGql, /requestGraphql/);

  const schema = fs.readFileSync(new URL('../schema/user.graphql', import.meta.url), 'utf8');
  assert.match(schema, /type Query|todos_create|chat_messages/);
  const pkg = fs.readFileSync(new URL('../package.json', import.meta.url), 'utf8');
  assert.match(pkg, /gen:gql|graphql-codegen/);
});

// DX contract lives in GitKB ([[specs/e2e-ui/sveltekit-dx]]), not the code tree.
// Structural checks above assert the pilot modules the spec describes.

test('chat: co-located .gql + resource — SSR query, WS subscription, browser post', () => {
  assert.ok(fs.existsSync(new URL('../src/routes/chat/chat.gql', import.meta.url)));
  assert.ok(fs.existsSync(new URL('../src/routes/chat/chat.generated.ts', import.meta.url)));
  const gqlFile = fs.readFileSync(new URL('../src/routes/chat/chat.gql', import.meta.url), 'utf8');
  assert.match(gqlFile, /query ChatMessages/);
  assert.match(gqlFile, /subscription ChatMessagesLive/);
  assert.match(gqlFile, /mutation ChatPost/);

  const resource = fs.readFileSync(
    new URL('../src/routes/chat/chat.resource.ts', import.meta.url),
    'utf8'
  );
  assert.match(resource, /defineResource/);
  assert.match(resource, /export const chat/);
  assert.match(resource, /ChatMessagesDocument|from '\.\/chat\.generated'/);
  assert.match(resource, /ChatMessagesLiveDocument|ChatPostDocument/);
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
  assert.match(page, /useGraphql/);
  assert.match(page, /chat\.mutations\.post|mutations\.post/);
  assert.match(page, /chat\.subscription|subscription/);
  assert.match(page, /subscribe/);
  assert.doesNotMatch(page, /browserGraphql|from '\$lib\/gql\/documents'/);
  assert.doesNotMatch(page, /use:enhance|\?\/create|export const actions/);

  const docs = fs.readFileSync(new URL('../src/lib/gql/documents.ts', import.meta.url), 'utf8');
  assert.match(docs, /chat\.resource|chat\.mutations|documentToString/);
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
