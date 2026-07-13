<script lang="ts">
	/**
	 * Distributed framework template landing — e2e fixture home.
	 */
	import { page } from '$app/state';
	import Footer from '$lib/components/shared/Footer.svelte';

	const session = $derived(page.data.session);
	const signedIn = $derived(!!session?.user);

	const demos = [
		{
			title: 'Owner-scoped todos',
			blurb:
				'Commands take identity from the access token; GraphQL filters owner_id to your sub. Optimistic UI with server validation.',
			where: 'ui/src/routes/todos · crates/todo-domain',
			href: '/todos',
			label: 'Open todos'
		},
		{
			title: 'Live lobby chat',
			blurb:
				'SSR seeds history; subscription { chat_messages } streams over /graphql/ws with Bearer in connection_init.',
			where: 'ui/src/routes/chat · crates/chat-domain',
			href: '/chat',
			label: 'Open chat'
		},
		{
			title: 'Session & tokens',
			blurb:
				'Inspect Auth.js session, access/id tokens, groups, and expiry — same OIDC path the API validates.',
			where: 'ui/src/routes/session · ui/src/auth.ts',
			href: '/session',
			label: 'Session inspector'
		},
		{
			title: 'Real OIDC (Zitadel)',
			blurb:
				'Docker IdP + Auth.js web app. make up bootstraps clients, humans, and machine keys into e2e-ui.env.',
			where: 'docker/ · scripts/up.sh · GET /signin',
			href: '/signin?callbackUrl=/todos',
			label: 'Sign in'
		},
		{
			title: 'SSR GraphQL',
			blurb:
				'Protected pages load on the server with the session access token — hard refresh paints data, not a client Loading spinner.',
			where: 'ui/src/lib/server/graphql.ts · +page.server.ts',
			href: '/todos',
			label: 'See SSR load'
		},
		{
			title: 'Multi-crate CQRS',
			blurb:
				'Pure domains, projectors-only read models, handlers in e2e-service. Suite runs offline SQLite or live Postgres + OIDC.',
			where: 'crates/* · e2e-suite · Makefile',
			href: '#code',
			label: 'Code samples'
		}
	];

	const sampleSsr = `// Load with Bearer — no client Loading flash
const session = await locals.auth();
const result = await serverGraphql(
  \`{ todos { todo_id title status } }\`,
  { accessToken: session?.accessToken }
);
return { todos: result.data?.todos ?? [] };`;

	const sampleWs = `// Browsers cannot set Authorization on upgrade
ws.onopen = () => {
  ws.send(JSON.stringify({
    type: 'connection_init',
    payload: {
      authorization: \`Bearer \${accessToken}\`
    }
  }));
};`;

	const sampleDomain = `// Owner is identity — never trusted from the body
pub fn create(
  &mut self,
  owner_id: &str,
  title: String,
) -> Result<(), TodoError> {
  self.record_created(owner_id, title)
}`;

	const sampleMake = `# Offline domain + behavioral + UI structural
make test

# Live OIDC + Postgres (stack must be up)
make test-live`;
</script>

<div class="df-home">
	<section class="df-hero">
		<div class="df-hero-inner">
			<span class="df-pill">Distributed · e2e-ui template</span>
			<h1>
				A <em>framework template</em> you run as full e2e tests — kept honest by the library.
			</h1>
			<p class="df-hero-lede">
				Copyable starting point for a Distributed CQRS service + SvelteKit UI: multi-domain models,
				GraphQL with row-level filters, WebSocket subscriptions, and real OIDC. The same folder is
				the living suite — <code>make test</code> offline, <code>make test-live</code> against
				Postgres + Zitadel.
			</p>
			<div class="df-actions">
				{#if signedIn}
					<a class="df-btn df-btn-primary" href="/todos">Open todos</a>
					<a class="df-btn df-btn-ghost" href="/chat">Lobby chat</a>
				{:else}
					<a class="df-btn df-btn-primary" href="/signin?callbackUrl=/todos">Sign in with OIDC</a>
					<a class="df-btn df-btn-ghost" href="#demos">What is demonstrated</a>
				{/if}
			</div>
			<div class="df-meta-row">
				<span>tests/e2e-ui</span>
				<span>API :8791</span>
				<span>UI :5180</span>
				<span>Zitadel :18080</span>
			</div>
		</div>
	</section>

	<section class="df-section" id="story">
		<div class="df-section-head">
			<h2>Template first, product second</h2>
			<p>
				This site is not a product marketing shell. It is the UI surface of a fixture that ships
				with the Distributed library: when patterns change, the e2e suite and this app move with
				them. Use it as a blueprint — domains stay pure, runner wires persistence + identity, UI
				proves the browser path.
			</p>
		</div>
		<div class="df-grid df-grid-3">
			<div class="df-card">
				<h3>Full e2e path</h3>
				<p>
					<code>make up</code> → Docker Postgres + Zitadel bootstrap. <code>make run</code> → API +
					Auth.js UI. Humans alice/bob for interactive login; machine keys for suite JWT-bearer.
				</p>
			</div>
			<div class="df-card">
				<h3>Updated with the library</h3>
				<p>
					Behavioral suite + gated OIDC tests live beside the service. Offline
					<code>make test</code> uses SQLite; live isolation needs the stack. Patterns here track
					framework defaults (OidcBearer, projectors, ChangeHub).
				</p>
			</div>
			<div class="df-card">
				<h3>Copy and extend</h3>
				<p>
					Multi-crate layout: todo-domain, chat-domain, readmodels, service, runner, suite. Swap
					DATABASE_URL / OIDC env for your IdP; keep handlers and UI routes as the map.
				</p>
			</div>
		</div>
	</section>

	<section class="df-band" id="run">
		<div class="df-section">
			<div class="df-section-head">
				<h2>Run the template</h2>
				<p>
					From <code>tests/e2e-ui</code> — full stack, then explore demos while signed in.
				</p>
			</div>
			<div class="df-steps">
				<div class="df-step">
					<h3>Bootstrap IdP + DB</h3>
					<p>
						<code>make up</code> writes <code>e2e-ui.env</code> with issuer, client, and machine
						keys.
					</p>
				</div>
				<div class="df-step">
					<h3>API + UI</h3>
					<p>
						<code>make run</code> serves GraphQL on :8791 and SvelteKit on :5180 with env loaded.
					</p>
				</div>
				<div class="df-step">
					<h3>Prove it</h3>
					<p>
						<code>make test</code> offline; <code>make test-live</code> for OIDC isolation. Demo
						password: <code>Password1!</code>
					</p>
				</div>
			</div>
		</div>
	</section>

	<section class="df-section" id="demos">
		<div class="df-section-head">
			<h2>What is demonstrated — and where</h2>
			<p>
				Each capability maps to a UI route or crate path. Click through after
				<code>make up && make run</code>.
			</p>
		</div>
		<div class="df-grid df-grid-2">
			{#each demos as d}
				<article class="df-card">
					<h3>{d.title}</h3>
					<span class="df-where">{d.where}</span>
					<p>{d.blurb}</p>
					<a class="df-card-link" href={d.href}>{d.label} →</a>
				</article>
			{/each}
		</div>
	</section>

	<section class="df-section" id="code">
		<div class="df-section-head">
			<h2>Simplicity in the hot paths</h2>
			<p>
				Short excerpts from this fixture — SSR GraphQL with the session token, WebSocket identity,
				and a domain command that never dual-writes.
			</p>
		</div>
		<div class="df-samples">
			<div class="df-code">
				<div class="df-code-bar">
					<span>ui/src/routes/todos/+page.server.ts</span>
					<em>SSR GraphQL</em>
				</div>
				<pre><code>{sampleSsr}</code></pre>
			</div>
			<div class="df-code">
				<div class="df-code-bar">
					<span>ui/src/lib/graphql-ws.ts</span>
					<em>WS auth</em>
				</div>
				<pre><code>{sampleWs}</code></pre>
			</div>
			<div class="df-code">
				<div class="df-code-bar">
					<span>crates/todo-domain · command</span>
					<em>CQRS</em>
				</div>
				<pre><code>{sampleDomain}</code></pre>
			</div>
			<div class="df-code">
				<div class="df-code-bar">
					<span>Makefile · suite</span>
					<em>e2e</em>
				</div>
				<pre><code>{sampleMake}</code></pre>
			</div>
		</div>
	</section>

	<section class="df-cta-band">
		<h2>Exercise the live demos</h2>
		<p>
			Sign in (alice / bob / admin · Password1!), open todos or chat, then inspect the session.
			GraphiQL stays on the API at <code>/graphql</code>.
		</p>
		<div class="df-actions">
			{#if signedIn}
				<a class="df-btn df-btn-primary" href="/todos">Todos</a>
				<a class="df-btn df-btn-ghost" href="/chat">Chat</a>
				<a class="df-btn df-btn-ghost" href="/session">Session</a>
			{:else}
				<a class="df-btn df-btn-primary" href="/signin?callbackUrl=/todos">Sign in</a>
				<a class="df-btn df-btn-ghost" href="/session">Session</a>
			{/if}
		</div>
	</section>

	<Footer />
</div>
