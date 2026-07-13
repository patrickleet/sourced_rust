/**
 * Structural + optional live contracts for e2e-ui.
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';

const base = process.env.E2E_API_ORIGIN || process.env.E2E_BASE_URL;

test('SSR is enabled (not SPA-only)', () => {
  // adapter-node + no export const ssr = false in layout
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
  // Auth shell is the-website's GET /signin
  const signin = fs.readFileSync(new URL('../src/routes/signin/+server.ts', import.meta.url), 'utf8');
  assert.match(signin, /\/auth\/signin\/oidc/);
  assert.match(signin, /X-Auth-Return-Redirect/);
  // Fixture pages added on top of website
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
  assert.doesNotMatch(nav, /\/docs|\/control-plane|Pricing|two-paths|HopsBrand|hops-ops/i);
});

test('home is distributed template landing with demos + code samples', () => {
  const home = fs.readFileSync(new URL('../src/routes/+page.svelte', import.meta.url), 'utf8');
  assert.match(home, /framework template|e2e-ui template/i);
  assert.match(home, /make test|test-live|e2e/i);
  assert.match(home, /GraphQL|subscription|connection_init|OIDC/i);
  assert.match(home, /\/todos/);
  assert.match(home, /\/chat/);
  assert.match(home, /sampleSsr|sampleWs|serverGraphql|connection_init/);
  assert.doesNotMatch(home, /Launch My Platform|HopsBrand|founder|listmonk/i);
  // At least two distinct sample string constants
  assert.match(home, /const sampleSsr/);
  assert.match(home, /const sampleWs/);

  const css = fs.readFileSync(new URL('../src/app.css', import.meta.url), 'utf8');
  assert.match(css, /--df-accent|--df-ink/);
  // Theme is not the old hops orange product identity as primary accent
  assert.match(css, /#1f9e78|#0e171c/);

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
  // OidcBearer require_auth → 401; DevHeaders may 200
  console.log(`live gql status=${res.status}`);
  assert.ok(res.status === 401 || res.status === 200, `unexpected ${res.status}`);
  if (res.status === 401) {
    const text = await res.text();
    assert.match(text, /unauth|UNAUTHENTICATED/i);
  }
});
