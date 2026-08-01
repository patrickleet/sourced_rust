<script lang="ts">
	/**
	 * Distributed product home — framework first.
	 * This SvelteKit app is the living playground; deep code walks live on each demo.
	 */
	import '$lib/styles/home.css';
	import { page } from '$app/state';
	import Footer from '$lib/components/shared/Footer.svelte';

	const session = $derived(page.data.session);
	const signedIn = $derived(!!session?.user);
	const authConfigError = $derived(page.url.searchParams.get('error') === 'Configuration');

	const pillars = [
		{
			title: 'Domain in Rust',
			body: 'Plain types, methods, and tests. Event sourcing and CQRS stay in the framework so your business rules stay readable.'
		},
		{
			title: 'GraphQL as the edge',
			body: 'Deny-by-default reads, first-class OIDC, application surfaces, and command mutations — not a second REST surface to keep in sync.'
		},
		{
			title: 'TypeScript clients',
			body: 'Generated operations and commands for SvelteKit: SSR hydrate, live subscriptions, causal optimism, and Projected boards from one inventory.'
		}
	];

	const story = [
		{
			n: '01',
			title: 'Write the domain once',
			body: 'Aggregates, events, and rules unit-test without HTTP or SQL. Macros record history; you keep the decisions.'
		},
		{
			n: '02',
			title: 'Register the service',
			body: 'Commands, projections, and read models live in one typed inventory. Surfaces (user, admin, public) select who may open what.'
		},
		{
			n: '03',
			title: 'Ship the UI',
			body: 'Co-locate GraphQL next to routes. Gen clients. The browser replica is the cache — optimism and live frames are not a second app.'
		}
	];

	const demos = [
		{ href: '/chat', title: 'Lobby chat', tag: 'Live + anonymous', blurb: 'SSR, @live, and guest reads on a public surface.' },
		{ href: '/todos', title: 'Todos', tag: 'Causal', blurb: 'Owner RLS, modeled optimism, projector fill.' },
		{ href: '/blob', tag: 'Projected', title: 'Blob game', blurb: 'Atomic board in the mutation payload.' },
		{ href: '/admin', title: 'Admin', tag: 'Surface', blurb: 'Second client for elevated ops.' },
		{ href: '/session', title: 'Session', tag: 'OIDC', blurb: 'Tokens, groups, and engine roles.' },
		{ href: '/public', title: 'Public', tag: 'Anonymous', blurb: 'Empty identity + named surface contract.' }
	];
</script>

<div class="wf-home dist-home">
	<section class="wf-hero">
		<div class="wf-hero-inner">
			{#if authConfigError}
				<div class="wf-auth-banner" role="alert">
					<strong>Identity provider unavailable</strong>
					<p>
						Sign-in needs Zitadel. From <code>tests/e2e-ui</code>: <code>make up</code>, then
						<code>source e2e-ui.env && make run</code>.
					</p>
				</div>
			{/if}

			<span class="wf-kicker">Distributed</span>
			<h1>
				Realtime GraphQL and a real domain — the next step after
				<em>query engines alone</em>.
			</h1>
			<p class="wf-lede">
				You already know the feel of live GraphQL: subscriptions, optimistic UI, one schema for the
				client. <strong>Distributed</strong> keeps that edge and adds what comes next — event-sourced
				commands in Rust, projections you own, OIDC surfaces, and generated TypeScript so the browser
				replica stays honest with the write model.
			</p>

			<ul class="wf-hero-stack" aria-label="Stack">
				<li>Rust domains</li>
				<li>GraphQL + OIDC</li>
				<li>TypeScript clients</li>
				<li>SvelteKit</li>
			</ul>

			<div class="wf-actions">
				{#if signedIn}
					<a class="wf-btn wf-btn-primary" href="/chat">Open the playground</a>
					<a class="wf-btn wf-btn-ghost" href="#try">Try demos</a>
				{:else}
					<a class="wf-btn wf-btn-primary" href="/chat">Browse the lobby</a>
					<a class="wf-btn wf-btn-ghost" href="/signin?callbackUrl=/todos">Sign in</a>
				{/if}
			</div>

			<p class="dist-hero-note">
				This site is the official <strong>living playground</strong> for Distributed — not a separate
				docs theme. Open any demo and use <em>How it’s built</em> for the code walkthrough.
			</p>
		</div>
	</section>

	<section class="wf-band wf-band-dark" id="what">
		<div class="wf-band-inner">
			<div class="wf-section-head">
				<span class="wf-label">What you get</span>
				<h2>Live reads — and a write side that scales with you</h2>
				<p>
					Realtime GraphQL solved the client story. The hard part was always the other half:
					commands, history, projections, and authz that don’t turn into a second product. Distributed
					is that evolution — one inventory from aggregate to SvelteKit, without locking you into a
					single opinionated app shape.
				</p>
			</div>
			<div class="dist-pillars">
				{#each pillars as p}
					<article class="dist-pillar">
						<h3>{p.title}</h3>
						<p>{p.body}</p>
					</article>
				{/each}
			</div>
		</div>
	</section>

	<section class="wf-band wf-band-light" id="flow">
		<div class="wf-band-inner">
			<div class="wf-section-head">
				<span class="wf-label">How it works</span>
				<h2>Domain → service → client</h2>
			</div>
			<ol class="dist-story">
				{#each story as step}
					<li>
						<span class="dist-story-n" aria-hidden="true">{step.n}</span>
						<div>
							<h3>{step.title}</h3>
							<p>{step.body}</p>
						</div>
					</li>
				{/each}
			</ol>
		</div>
	</section>

	<section class="wf-band wf-band-dark" id="try">
		<div class="wf-band-inner">
			<div class="wf-section-head">
				<span class="wf-label">Playground</span>
				<h2>Try it here</h2>
				<p>
					These routes are real Distributed apps under <code>tests/e2e-ui</code>. Each one has a
					<strong>How it’s built</strong> panel — browser query, commands, handlers, domain, events,
					and RBAC.
				</p>
			</div>
			<div class="dist-demo-grid">
				{#each demos as d}
					<a class="dist-demo-card" href={d.href}>
						<span class="dist-demo-tag">{d.tag}</span>
						<h3>{d.title}</h3>
						<p>{d.blurb}</p>
						<span class="dist-demo-go">Open →</span>
					</a>
				{/each}
			</div>
		</div>
	</section>

	<section class="wf-band wf-band-light" id="run">
		<div class="wf-band-inner dist-run">
			<div class="wf-section-head">
				<span class="wf-label">Local</span>
				<h2>Run the playground</h2>
				<p>From the repository:</p>
			</div>
			<div class="dist-run-code">
				<pre><code>{`cd tests/e2e-ui
make up                    # Postgres + Zitadel → e2e-ui.env
source e2e-ui.env && make run
# UI  http://localhost:5180
# API http://127.0.0.1:8791`}</code></pre>
			</div>
			<p class="dist-run-hint">
				Demo logins after <code>make up</code>: <code>alice</code> / <code>bob</code> /
				<code>admin</code> · <code>Password1!</code>
			</p>
		</div>
	</section>

	<section class="wf-band wf-band-dark dist-closing">
		<div class="wf-band-inner">
			<div class="wf-section-head">
				<span class="wf-label">Next</span>
				<h2>Build on Distributed</h2>
				<p>
					Copy the patterns from this playground into your service. Fleet hosting
					(<strong>ops.com.ai</strong>) is on the roadmap; for now the open-source framework and this
					fixture are the product surface.
				</p>
			</div>
			<div class="wf-actions">
				<a class="wf-btn wf-btn-primary" href="/chat">Start with chat</a>
				<a class="wf-btn wf-btn-ghost" href="/todos">Or todos</a>
			</div>
		</div>
	</section>

	<Footer />
</div>
