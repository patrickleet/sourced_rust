<script lang="ts">
	/**
	 * Distributed framework template home.
	 * Full unidirectional todos story: OIDC → SSR GQL → hydrate → subscribe →
	 * command mutation → aggregate → project → read.
	 */
	import { page } from '$app/state';
	import Footer from '$lib/components/shared/Footer.svelte';

	const session = $derived(page.data.session);
	const signedIn = $derived(!!session?.user);
	/** Auth.js bounce when OIDC/IdP is misconfigured or Zitadel is down. */
	const authConfigError = $derived(page.url.searchParams.get('error') === 'Configuration');

	/** One file / one concept per code block — never merge paths in a single sample. */
	type CodeBlock = { file: string; label: string; code: string };

	const demos = [
		{
			href: '/todos',
			title: 'Todos — owner-scoped field notes',
			blurb:
				'SSR GraphQL with row-level RBAC, progressive form actions, optimistic UI over projector lag. The full unidirectional path below is this feature.',
			where: '/todos · GraphQL query + todos_* mutations',
			label: 'Open todos'
		},
		{
			href: '/chat',
			title: 'Lobby chat — live subscription',
			blurb:
				'Same GraphQL selection shape as a query, with subscription + connection_init Bearer. Shared room; messages project into chat_messages.',
			where: '/chat · graphql-ws · project_chat',
			label: 'Open chat'
		},
		{
			href: '/session',
			title: 'Session inspector',
			blurb:
				'What Auth.js put in the encrypted cookie: access token present, sub, groups/roles mapped for the engine.',
			where: '/session · locals.auth()',
			label: 'Inspect session'
		}
	];

	const steps: Array<{
		n: string;
		title: string;
		why: string;
		path: string;
		label: string;
		blocks: CodeBlock[];
	}> = [
		{
			n: '01',
			title: 'Login via custom pages + Zitadel — session holds the access token',
			why: 'Auth.js starts OIDC (PKCE); Zitadel redirects to our /login form; Session API + CreateCallback return the code; Auth.js stores tokens in an encrypted httpOnly cookie for SSR and GraphQL.',
			path: 'ui/src/routes/login · auth.ts · Session API v2',
			label: 'Auth session',
			blocks: [
				{
					file: 'ui/src/auth.ts',
					label: 'Auth session',
					code: `// Auth.js OIDC (Zitadel) — PKCE + state
// Tokens live in an encrypted JWT session cookie (httpOnly)
callbacks: {
  async jwt({ token, account }) {
    if (account) {
      token.accessToken = account.access_token;
      token.refreshToken = account.refresh_token;
      token.idToken = account.id_token;
      token.expiresAt = account.expires_at ?? …;
    }
    // silent refresh before expiry when refresh_token present
    return token;
  },
  async session({ session, token }) {
    session.accessToken = token.accessToken; // API Bearer
    session.user.id = token.sub;
    return session;
  }
}`
				}
			]
		},
		{
			n: '02',
			title: 'SSR runs a GraphQL query with RBAC',
			why: 'The todos page load() calls the API with Authorization: Bearer <accessToken>. The engine validates the JWT (OidcBearer), maps claims to identity, and applies ModelPermissions so a user only sees rows where owner_id = their sub.',
			path: 'todos/+page.server.ts · graphql.ts · service.rs',
			label: 'SSR + RBAC',
			blocks: [
				{
					file: 'todos/+page.server.ts',
					label: 'SSR load',
					code: `const session = await locals.auth();
const result = await serverGraphql(
  \`{
    todos {
      todo_id
      owner_id
      title
      status
    }
  }\`,
  { accessToken: session?.accessToken }
);`
				},
				{
					file: 'ui/src/lib/server/graphql.ts',
					label: 'Bearer auth',
					code: `// How GQL is authenticated on every SSR call
if (opts.accessToken) {
  headers.authorization = \`Bearer \${opts.accessToken}\`;
}
await fetch(\`\${apiBase()}/graphql\`, {
  method: 'POST',
  headers,
  body: JSON.stringify({ query })
});`
				},
				{
					file: 'crates/service/src/service.rs',
					label: 'Row-level RBAC',
					code: `// OidcBearer validates JWT → x-user-id (sub) + engine roles
.model::<TodoView>(
  ModelPermissions::new()
    .role("user", select().all_columns().filter(
      col("owner_id").eq(claim("x-user-id")),
    ))
    .role("admin", select().all_columns()),
)`
				}
			]
		},
		{
			n: '03',
			title: 'Hydration matches SSR — no empty flash',
			why: 'Server HTML already contains the list. The client seeds state from data.todos instead of starting at []. Re-loads merge server data instead of wiping the DOM to empty first.',
			path: 'ui/src/routes/todos/+page.svelte',
			label: 'Hydration',
			blocks: [
				{
					file: 'todos/+page.svelte',
					label: 'Hydration',
					code: `// Seed client state from SSR props so first paint matches
let todos = $state<Todo[]>([...(data.todos ?? [])]);

// When load() re-runs after a mutation, merge — don't start from []
$effect(() => {
  const server = data.todos;
  untrack(() => mergeFromServer(server));
});`
				}
			]
		},
		{
			n: '04',
			title: 'Same fields as a live subscription',
			why: 'Change query to subscription for push updates. Identity rides in connection_init (not the WebSocket upgrade headers), which is the browser-safe OIDC pattern.',
			path: 'subscription · graphql-ws.ts',
			label: 'Subscription',
			blocks: [
				{
					file: 'GraphQL subscription',
					label: 'Same fields',
					code: `// Same selection as the SSR query — change query → subscription
// (chat page uses this pattern live; todos can too)
subscription {
  todos {
    todo_id
    owner_id
    title
    status
  }
}`
				},
				{
					file: 'ui/src/lib/graphql-ws.ts',
					label: 'WS auth',
					code: `// Browser WS cannot set Authorization on the upgrade handshake.
// Auth goes in connection_init:
ws.send(JSON.stringify({
  type: 'connection_init',
  payload: { authorization: \`Bearer \${accessToken}\` }
}));`
				}
			]
		},
		{
			n: '05',
			title: 'Mutations that write the read model are an anti-pattern',
			why: 'Direct GraphQL updates to the todos table invent a second source of truth. Writes are typed command mutations (todos_create → todo.create, …), role-gated to user/admin. Owner always comes from the session. There is no public POST /todo.* — GraphQL only.',
			path: 'todos_* mutations · without_http_command_routes',
			label: 'Commands not RM writes',
			blocks: [
				{
					file: 'GraphQL mutation',
					label: 'todos_create',
					code: `// Anti-pattern: mutation that UPDATEs todos rows.
// Command mutation (dispatches todo.create handler):
mutation {
  todos_create(input: { todo_id: "t-1", title: "Ship it" }) {
    todo_id
    owner_id
    title
    status
  }
}
// Also: todos_complete, todos_archive, todos_rename, …
// Roles: user, admin. owner_id is NOT in input.`
				},
				{
					file: 'crates/service/src/service.rs',
					label: 'GraphQL-only surface',
					code: `Service::new()
  .named("e2e-ui")
  .without_http_command_routes() // no POST /todo.*
  .routes(todos)
  .routes(chat);

// Mutations register the same handlers:
// todos_create → todo.create (owner = session)
// todos_complete / archive / rename / reopen
// chat_messages_post → chat.post`
				},
				{
					file: 'handlers/commands/create.rs',
					label: 'Owner from session',
					code: `// Owner is always the authenticated principal
let owner = require_user(ctx.session())?;
let input = ctx.input::<TodoCreateInput>()?;
todo.create(&input.todo_id, &owner, &input.title)?;`
				},
				{
					file: 'ui · browser + SSR',
					label: 'Same documents',
					code: `// todos.gql → codegen → todos.resource (defineResource)
// SSR:  loadQuery(todos.query, …)
// Browser: useGraphql(() => data).request(todos.mutations.create, vars)
// → POST /graphql (not SvelteKit form actions)`
				}
			]
		},
		{
			n: '06',
			title: 'Handler updates the aggregate and publishes a domain event',
			why: 'After authentication, the command handler calls Todo::create, commits the event store, and enqueues an outbox fact (todo.created). The read model is not touched here.',
			path: 'crates/service/src/handlers/commands/create.rs',
			label: 'Command handler',
			blocks: [
				{
					file: 'handlers/commands/create.rs',
					label: 'Command handler',
					code: `// After OidcBearer / session auth — never touches the todos table.
pub async fn handle(ctx: &Context<'_, TodoDeps<…>>)
  -> Result<Value, HandlerError>
{
  let owner = require_user(ctx.session())?;
  let input = ctx.input::<Input>()?;

  let mut todo = Todo::default();
  todo.create(&input.todo_id, &owner, &input.title)?;

  let fact = TodoFact::from_todo(&todo);
  let outbox = OutboxMessage::encode(
    format!("{}:todo.created:{}", todo.todo_id, …),
    "todo.created",
    &fact,
  )?;
  ctx.repo().outbox(outbox).commit(&mut todo).await?;
  Ok(json!({ "todo_id": fact.todo_id, "status": fact.status }))
}`
				}
			]
		},
		{
			n: '07',
			title: 'Projection handler applies the fact to the read model',
			why: 'An event consumer runs project_todo for todo.* facts, maps TodoFact → TodoView, and upserts the todos table. That is the only write path for queryable rows.',
			path: 'crates/service/src/handlers/events/project_todo.rs',
			label: 'Projector',
			blocks: [
				{
					file: 'handlers/events/project_todo.rs',
					label: 'Projector',
					code: `// Only projector writes the read model (commands never do).
pub const EVENTS: &[&str] = &[
  "todo.created", "todo.renamed", "todo.completed",
  "todo.reopened", "todo.archived",
];

pub async fn handle(ctx: &Context<'_, TodoDeps<…>>)
  -> Result<Value, HandlerError>
{
  let fact: TodoFact = decode_payload(ctx.message())?;
  let row = map_fact(&fact); // → TodoView
  let mut plan = ReadModelWritePlanBuilder::new();
  plan.upsert(&row)?;
  plan.commit(ctx.read_model_store()).await?;
  Ok(json!({ "todo_id": fact.todo_id, "status": fact.status }))
}`
				}
			]
		},
		{
			n: '08',
			title: 'Queries and subscriptions observe the new state',
			why: 'The loop closes: the next query and any open subscription deliver the projected row. One direction only — UI never patches the read model as truth.',
			path: 'query · subscription · ChangeHub',
			label: 'Read path',
			blocks: [
				{
					file: 'GraphQL query',
					label: 'Next read',
					code: `// After the projector commits TodoView:
query {
  todos {
    todo_id
    owner_id
    title
    status
  }
}`
				},
				{
					file: 'GraphQL subscription',
					label: 'Live push',
					code: `// Open WS clients get the same row via ChangeHub
subscription {
  todos {
    todo_id
    owner_id
    title
    status
  }
}`
				},
				{
					file: 'Unidirectional flow',
					label: 'One direction',
					code: `// Never: GraphQL mutation rewriting todos as truth.
//
// UI ──Bearer──► command | GraphQL query/sub
//        │
//        ▼
// handler → aggregate + outbox event
//        │
//        ▼
// projector → read model (todos table)
//        │
//        ▼
// query / subscription results`
				}
			]
		}
	];
</script>

<div class="wf-home">
	<!-- Hero: allowed soft/transparent grid treatment -->
	<section class="wf-hero">
		<div class="wf-hero-inner">
			{#if authConfigError}
				<div class="wf-auth-banner" role="alert">
					<strong>Identity provider unavailable</strong>
					<p>
						Sign-in / create-account need Zitadel. Docker was not reachable (or OIDC env is
						missing), so Auth.js returned <code>error=Configuration</code>.
					</p>
					<ol>
						<li>Start Docker / Colima</li>
						<li>
							<code>cd tests/e2e-ui && make up</code> — boots Postgres + Zitadel and enables
							self-registration
						</li>
						<li>
							<code>source e2e-ui.env && make run</code> — API + UI with OIDC
						</li>
						<li>
							Demo logins: <code>alice</code> / <code>bob</code> / <code>admin</code> ·
							<code>Password1!</code>
						</li>
					</ol>
				</div>
			{/if}
			<span class="wf-kicker">Distributed · e2e-ui template</span>
			<h1>
				A <em>framework template</em> you run as full e2e tests — kept honest by the library.
			</h1>
			<p class="wf-lede">
				Multi-crate CQRS, GraphQL with row-level filters, WebSocket subscriptions, real OIDC. The
				same folder is the living suite — <code>make test</code> offline,
				<code>make test-live</code> against Postgres + Zitadel.
			</p>
			<div class="wf-actions">
				{#if signedIn}
					<a class="wf-btn wf-btn-primary" href="/todos">Open todos</a>
					<a class="wf-btn wf-btn-ghost" href="/chat">Lobby chat</a>
				{:else}
					<a class="wf-btn wf-btn-primary" href="/signin?callbackUrl=/todos">Sign in with OIDC</a>
					<a class="wf-btn wf-btn-ghost" href="#story-flow">Unidirectional story</a>
				{/if}
			</div>
			<div class="wf-meta">
				<span>tests/e2e-ui</span>
				<span>API :8791</span>
				<span>UI :5180</span>
				<span>Zitadel :18080</span>
			</div>
		</div>
	</section>

	<!-- dark first after light hero -->
	<section class="wf-band wf-band-dark" id="story">
		<div class="wf-band-inner">
			<div class="wf-section-head">
				<span class="wf-label">Why this exists</span>
				<h2>Template first. Product later.</h2>
				<p>
					Not a product marketing site — the UI face of a fixture that ships with Distributed. When
					patterns change, the suite and this app move with them.
				</p>
			</div>
			<div class="wf-cards">
				<div class="wf-card">
					<h3>Full e2e path</h3>
					<p>
						<code>make up</code> boots Postgres + Zitadel. <code>make run</code> serves API + UI.
						Humans alice/bob for login; machine keys for suite JWT-bearer.
					</p>
				</div>
				<div class="wf-card">
					<h3>Updated with the library</h3>
					<p>
						Behavioral + gated OIDC tests live beside the service. Offline SQLite or live stack.
						OidcBearer, projectors, ChangeHub track framework defaults.
					</p>
				</div>
				<div class="wf-card">
					<h3>Copy and extend</h3>
					<p>
						todo-domain, chat-domain, readmodels, service, runner, suite. Swap DATABASE_URL / OIDC
						env; keep handlers and routes as the map.
					</p>
				</div>
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
					<h3>Bootstrap IdP + DB</h3>
					<p><code>make up</code> writes <code>e2e-ui.env</code>.</p>
				</div>
				<div class="wf-step">
					<h3>API + UI</h3>
					<p><code>make run</code> — GraphQL :8791, SvelteKit :5180.</p>
				</div>
				<div class="wf-step">
					<h3>Prove it</h3>
					<p><code>make test</code> · <code>make test-live</code>.</p>
				</div>
			</div>
		</div>
	</section>

	<section class="wf-band wf-band-dark" id="demos">
		<div class="wf-band-inner">
			<div class="wf-section-head">
				<span class="wf-label">Live demos</span>
				<h2>What is demonstrated — and where</h2>
				<p>Open after <code>make up && make run</code>.</p>
			</div>
			<div class="wf-demos">
				{#each demos as d, i}
					<a class="wf-demo" href={d.href}>
						<span class="wf-demo-i">{String(i + 1).padStart(2, '0')}</span>
						<div>
							<h3>{d.title}</h3>
							<p>{d.blurb}</p>
							<div class="wf-demo-where">{d.where}</div>
						</div>
						<span class="wf-demo-go">{d.label} →</span>
					</a>
				{/each}
			</div>
		</div>
	</section>

	<section class="wf-band wf-band-light" id="story-flow">
		<div class="wf-band-inner">
			<div class="wf-section-head">
				<span class="wf-label">Todos · unidirectional system</span>
				<h2>What this is demonstrating</h2>
				<p>
					A single feature path from login to live reads — one direction only. Each step below is
					grounded in this fixture’s real files.
				</p>
			</div>
			<ol class="wf-flow-map">
				<li>Login (Zitadel + Auth.js session)</li>
				<li>SSR GraphQL query + RBAC + Bearer</li>
				<li>Hydrate client = SSR HTML</li>
				<li>Optional: subscription on the same fields</li>
				<li>Command (not RM mutation) + RBAC</li>
				<li>Handler → aggregate → domain event</li>
				<li>Projector → read model</li>
				<li>Queries &amp; subscriptions observe</li>
			</ol>
		</div>
	</section>

	<!-- Step bands continue dark / light after light story-flow intro -->
	{#each steps as step, i}
		<section
			class="wf-band {i % 2 === 0 ? 'wf-band-dark' : 'wf-band-light'}"
			id="step-{step.n}"
			data-sample={step.label}
		>
			<div class="wf-band-inner wf-story-step">
				<div class="wf-story-copy">
					<span class="wf-label">Step {step.n}</span>
					<h2>{step.title}</h2>
					<p class="wf-why">{step.why}</p>
					<span class="wf-sample-path">{step.path}</span>
				</div>
				<div class="wf-code-stack">
					{#each step.blocks as block}
						<div class="wf-code">
							<div class="wf-code-bar">
								<span>{block.file}</span>
								<em>{block.label}</em>
							</div>
							<pre><code>{block.code}</code></pre>
						</div>
					{/each}
				</div>
			</div>
		</section>
	{/each}

	<!-- CTA solid dark -->
	<section class="wf-band wf-band-dark wf-cta-band">
		<div class="wf-band-inner wf-cta">
			<h2>Exercise the live demos</h2>
			<p>
				Sign in (alice / bob / admin · Password1!), open todos or chat, then inspect the session.
			</p>
			<div class="wf-actions">
				{#if signedIn}
					<a class="wf-btn wf-btn-primary" href="/todos">Todos</a>
					<a class="wf-btn wf-btn-ghost" href="/chat">Chat</a>
					<a class="wf-btn wf-btn-ghost" href="/session">Session</a>
				{:else}
					<a class="wf-btn wf-btn-primary" href="/signin?callbackUrl=/todos">Sign in</a>
					<a class="wf-btn wf-btn-ghost" href="/session">Session</a>
				{/if}
			</div>
		</div>
	</section>

	<Footer />
</div>
