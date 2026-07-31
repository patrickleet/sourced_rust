<script lang="ts">
	/**
	 * Home — Distributed framework first; demos are destinations.
	 * Per-demo “How it’s built” slide-outs live on each route.
	 */
	import '$lib/styles/home.css';
	import { page } from '$app/state';
	import Footer from '$lib/components/shared/Footer.svelte';

	const session = $derived(page.data.session);
	const signedIn = $derived(!!session?.user);
	const authConfigError = $derived(page.url.searchParams.get('error') === 'Configuration');

	const frameworkPrinciples = [
		{
			title: 'Simplest DX is the goal',
			body: 'Event sourcing and CQRS are easy to overbuild. Distributed’s job is the opposite: plain domain intent and ordinary UI, while the library carries history, projection, GraphQL, and the browser replica.'
		},
		{
			title: 'Start with the domain, not the database',
			body: 'Model a todo or game as a plain Rust type with methods and unit tests. No HTTP, no SQL, no handler context to prove the rules. Infrastructure plugs in after the behavior is solid.'
		},
		{
			title: 'Know which side of the fence you are on',
			body: 'Aggregate events replay write-side history. Domain events are the outward contract. Read models stay query-shaped — never dual-write tables from the UI.'
		},
		{
			title: 'You keep the interesting code',
			body: 'Macros own recording facts and replaying history so methods stay readable. You still own “only the owner can complete this.” Scaffolding disappears; decisions do not.'
		},
		{
			title: 'Register once, ship everywhere',
			body: 'Commands, domain events, and projections live once in the typed Service inventory. Generation lowers that into GraphQL, typed UI helpers, safe optimism, and dual application surfaces. Drift fails CI on purpose.'
		},
		{
			title: 'Grow without rewriting what you proved',
			body: 'Start as one process on a laptop. Later split services or change brokers. Domain types and facts you already tested should not need a rewrite.'
		}
	];

	const demos = [
		{
			href: '/todos',
			title: 'Todos',
			tag: 'Causal',
			blurb: 'Owner-scoped list, modeled optimism, projector fill.'
		},
		{
			href: '/chat',
			title: 'Lobby chat',
			tag: '@load @live',
			blurb: 'One query for SSR and live frames; posts are commands.'
		},
		{
			href: '/blob',
			title: 'Blob game',
			tag: 'Projected',
			blurb: 'Board commits with the event — no dual-write lag.'
		},
		{
			href: '/admin',
			title: 'Admin',
			tag: 'Surface',
			blurb: 'Separate generated client for elevated ops.'
		},
		{
			href: '/session',
			title: 'Session',
			tag: 'OIDC',
			blurb: 'Who am I to the API — roles and tokens.'
		},
		{
			href: '/public',
			title: 'Public',
			tag: 'Anonymous',
			blurb: 'Empty identity + named public surface.'
		}
	];

	const crates = [
		{ name: '*-domain', role: 'Aggregates, events, DomainState' },
		{ name: 'e2e-readmodels', role: 'Query shapes + model RBAC' },
		{ name: 'e2e-projections', role: 'Event → table programs' },
		{ name: 'e2e-service', role: 'Handlers, surfaces, GraphQL' },
		{ name: 'e2e-runner', role: 'Process on :8791' },
		{ name: 'ui/', role: 'SvelteKit · OIDC · demos' }
	];
</script>

<div class="wf-home">
	<section class="wf-hero">
		<div class="wf-hero-inner">
			{#if authConfigError}
				<div class="wf-auth-banner" role="alert">
					<strong>Identity provider unavailable</strong>
					<p>
						Sign-in needs Zitadel. Run <code>make up</code> then
						<code>source e2e-ui.env && make run</code>.
					</p>
				</div>
			{/if}
			<span class="wf-kicker">Distributed · full stack</span>
			<h1>
				Full-stack <em>CQRS</em> — Rust domains, GraphQL, and <em>TypeScript</em> clients with
				first-class <em>OIDC</em> and <em>SvelteKit</em>.
			</h1>
			<p class="wf-lede">
				One inventory from command to browser replica: event-sourced write models, deny-by-default
				GraphQL reads, generated TS operations and causal commands, Auth.js + IdP sign-in, and a
				SvelteKit app that SSR-hydrates and stays live over WebSocket. This fixture is the map —
				not a product marketing site.
			</p>
			<ul class="wf-hero-stack" aria-label="Stack highlights">
				<li>CQRS + event sourcing</li>
				<li>TypeScript clients</li>
				<li>First-class OIDC</li>
				<li>SvelteKit SSR + live</li>
			</ul>
			<div class="wf-actions">
				{#if signedIn}
					<a class="wf-btn wf-btn-primary" href="#demos">Open a demo</a>
					<a class="wf-btn wf-btn-ghost" href="#principles">Principles</a>
				{:else}
					<a class="wf-btn wf-btn-primary" href="/signin?callbackUrl=/todos">Sign in with OIDC</a>
					<a class="wf-btn wf-btn-ghost" href="#principles">Principles</a>
				{/if}
			</div>
			<div class="wf-meta">
				<span>Rust service</span>
				<span>GraphQL · OIDC</span>
				<span>TS · SvelteKit</span>
				<span>:8791 · :5180</span>
			</div>
			<nav class="wf-toc" aria-label="On this page">
				<a href="#principles">Principles</a>
				<a href="#dx">DX</a>
				<a href="#demos">Demos</a>
				<a href="#run">Run</a>
				<a href="#map">Crate map</a>
			</nav>
		</div>
	</section>

	<section class="wf-band wf-band-dark" id="principles">
		<div class="wf-band-inner">
			<div class="wf-section-head wf-section-head-wide">
				<span class="wf-label">Framework principles</span>
				<h2>What we optimize for</h2>
				<p>
					Not “more infrastructure.” The north star is the
					<strong>simplest path</strong> that keeps write history, queries, and published messages
					honest — so you can grow later without rewriting the domain you already proved.
				</p>
			</div>
			<ol class="wf-principles">
				{#each frameworkPrinciples as p, i}
					<li class="wf-principle">
						<span class="wf-principle-n" aria-hidden="true">{String(i + 1).padStart(2, '0')}</span>
						<div>
							<h3>{p.title}</h3>
							<p>{p.body}</p>
						</div>
					</li>
				{/each}
			</ol>
		</div>
	</section>

	<section class="wf-band wf-band-light" id="dx">
		<div class="wf-band-inner">
			<div class="wf-section-head">
				<span class="wf-label">How DX stays simple</span>
				<h2>Three layers carry the weight</h2>
			</div>
			<div class="wf-layers">
				<div class="wf-layer">
					<span class="wf-layer-n" aria-hidden="true">01</span>
					<h4>You write plain domain code</h4>
					<p class="wf-layer-body">
						Ordinary methods and tests. Public commands enforce rules; private
						<code>#[event]</code> helpers record history.
					</p>
				</div>
				<div class="wf-layer">
					<span class="wf-layer-n" aria-hidden="true">02</span>
					<h4>One inventory, many surfaces</h4>
					<p class="wf-layer-body">
						Describe commands, events, and projections once. Generation expands GraphQL, typed UI
						helpers, optimism, and dual application clients.
					</p>
				</div>
				<div class="wf-layer">
					<span class="wf-layer-n" aria-hidden="true">03</span>
					<h4>Short, swappable verbs</h4>
					<p class="wf-layer-body">
						<code>get</code> / <code>create</code> / <code>publish_events</code> /
						<code>project</code> / <code>commit</code> → <code>Causal</code> or
						<code>Projected</code>. Swap memory for Postgres without rewriting the domain.
					</p>
				</div>
			</div>

			<div class="wf-subhead">
				<span class="wf-label">Shape of a feature</span>
				<h3>Prove the domain before the plumbing</h3>
			</div>
			<ol class="wf-flow-map">
				<li>Unit-test the model</li>
				<li>Implement the plain type</li>
				<li>Command handler → fluent commit</li>
				<li>Projection program (eventual or direct)</li>
				<li>Read RBAC on the query model</li>
				<li>Co-located <code>+page.graphql</code> · gen-client · thin UI</li>
			</ol>
		</div>
	</section>

	<section class="wf-band wf-band-dark" id="demos">
		<div class="wf-band-inner">
			<div class="wf-section-head">
				<span class="wf-label">Practice arenas</span>
				<h2>Demos</h2>
				<p>
					Each route is a small story about one way the stack behaves. Open it, then use
					<strong>How it’s built</strong> (bottom-right) for a tabbed walkthrough of domain →
					command → projection → client.
				</p>
			</div>
			<div class="wf-demos wf-demos-compact">
				{#each demos as d, i}
					<a class="wf-demo" href={d.href}>
						<span class="wf-demo-i">{String(i + 1).padStart(2, '0')}</span>
						<div>
							<div class="wf-demo-title-row">
								<h3>{d.title}</h3>
								<span class="wf-demo-tag">{d.tag}</span>
							</div>
							<p>{d.blurb}</p>
						</div>
						<span class="wf-demo-go">Open →</span>
					</a>
				{/each}
			</div>
		</div>
	</section>

	<section class="wf-band wf-band-light" id="run">
		<div class="wf-band-inner">
			<div class="wf-section-head">
				<span class="wf-label">Run</span>
				<h2>Three commands. Full stack.</h2>
				<p>
					From <code>tests/e2e-ui</code>. Demo password: <code>Password1!</code> (alice / bob /
					admin).
				</p>
			</div>
			<div class="wf-steps">
				<div class="wf-step">
					<h3>Bootstrap</h3>
					<p><code>make up</code> — Postgres + Zitadel → <code>e2e-ui.env</code></p>
				</div>
				<div class="wf-step">
					<h3>API + UI</h3>
					<p><code>source e2e-ui.env && make run</code> — :8791 / :5180</p>
				</div>
				<div class="wf-step">
					<h3>Prove it</h3>
					<p><code>make test</code> · <code>make test-browser</code> · <code>make check-client</code></p>
				</div>
			</div>
		</div>
	</section>

	<section class="wf-band wf-band-dark" id="map">
		<div class="wf-band-inner">
			<div class="wf-section-head">
				<span class="wf-label">Repository</span>
				<h2>Where to look</h2>
			</div>
			<dl class="wf-crate-map">
				{#each crates as c}
					<div class="wf-crate">
						<dt>{c.name}</dt>
						<dd>{c.role}</dd>
					</div>
				{/each}
			</dl>
			<div class="wf-code wf-code-lead" style="margin-top: 2rem">
				<div class="wf-code-bar">
					<span>Mental model</span>
					<em>system</em>
				</div>
				<pre><code>{`Browser
  OIDC session → Bearer / WS connection_init
  @load SSR hydrate → one replica
  commands.* → Causal or Projected

Service
  OidcBearer → x-user-id + x-roles (set)
  surface privilege pack for execution
  inventory → GraphQL + dual clients
       todos/chat: Causal + projector
       blob:       Projected (same txn)
  auth_users from Zitadel for joins`}</code></pre>
			</div>
		</div>
	</section>

	<section class="wf-band wf-band-light">
		<div class="wf-band-inner">
			<div class="wf-section-head">
				<span class="wf-label">Next</span>
				<h2>Pick a demo and open the drawer</h2>
				<p>
					The long walkthroughs moved into each route’s <strong>How it’s built</strong> panel —
					tabs for domain, command, projection, client, and the principle each layer exercises.
				</p>
			</div>
			<div class="wf-actions">
				<a class="wf-btn wf-btn-primary" href="/todos">Start with todos</a>
				<a class="wf-btn wf-btn-ghost" href="/chat">Or lobby chat</a>
			</div>
		</div>
	</section>

	<Footer />
</div>
