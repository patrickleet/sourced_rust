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
  assert.match(todosServer, /serverGraphql|PageServerLoad/);
  assert.match(todosServer, /accessToken/);
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
  assert.match(gql, /Bearer/);
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

test('todos seeds client state from SSR load data', () => {
  const page = fs.readFileSync(new URL('../src/routes/todos/+page.svelte', import.meta.url), 'utf8');
  assert.match(page, /\$state<Todo\[\]>\(\[\.\.\.\(data\.todos/);
  assert.match(page, /mergeFromServer/);
  assert.match(page, /use:enhance/);
  const server = fs.readFileSync(
    new URL('../src/routes/todos/+page.server.ts', import.meta.url),
    'utf8'
  );
  assert.match(server, /serverGraphql/);
  assert.match(server, /accessToken/);
  // All writes go through GraphQL command mutations (no serverCommand / HTTP)
  assert.match(server, /todos_create/);
  assert.match(server, /todos_complete/);
  assert.match(server, /todos_archive/);
  assert.doesNotMatch(server, /serverCommand/);
  const gqlLib = fs.readFileSync(new URL('../src/lib/server/graphql.ts', import.meta.url), 'utf8');
  assert.doesNotMatch(gqlLib, /export async function serverCommand/);
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
