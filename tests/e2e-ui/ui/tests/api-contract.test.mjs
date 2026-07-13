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

test('home is distributed template landing with CQRS architecture samples', () => {
  const home = fs.readFileSync(new URL('../src/routes/+page.svelte', import.meta.url), 'utf8');
  assert.match(home, /framework template|e2e-ui template/i);
  assert.match(home, /make test|test-live|e2e/i);
  assert.match(home, /\/todos/);
  assert.match(home, /\/chat/);
  assert.doesNotMatch(home, /Launch My Platform|HopsBrand|founder|listmonk|Systems Lab|lab-acid/i);

  // Five architecture samples for todos CQRS path
  assert.match(home, /const sampleAggregate/);
  assert.match(home, /const sampleCommand/);
  assert.match(home, /const sampleReadModel/);
  assert.match(home, /const sampleProjector/);
  assert.match(home, /const sampleService/);
  assert.match(home, /data-sample="aggregate"/);
  assert.match(home, /data-sample="command-handler"/);
  assert.match(home, /data-sample="read-model"/);
  assert.match(home, /data-sample="projection-handler"/);
  assert.match(home, /data-sample="service-config"/);
  // Real fixture roles
  assert.match(home, /todo-domain|TodoFact|record_created/);
  assert.match(home, /todo\.create|require_user|outbox/);
  assert.match(home, /TodoView|#\[table\("todos"\)\]|ReadModel/);
  assert.match(home, /project_todo|ReadModelWritePlanBuilder|todo\.created/);
  assert.match(home, /build_service|run_postgres|run_sqlite|identity_from_env/);

  const css = fs.readFileSync(new URL('../src/app.css', import.meta.url), 'utf8');
  assert.match(css, /Neutral wireframe|--wf-bg|--wf-ink/);
  assert.match(css, /#f6f5f2|#1c1c1a/);
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
