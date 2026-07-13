<script lang="ts">
	/**
	 * Distributed framework template — neutral wireframe home.
	 * Architecture samples mirror real crates under tests/e2e-ui.
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
				'SSR seeds history; subscription { chat_messages } over /graphql/ws with Bearer in connection_init.',
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
				'Docker IdP + Auth.js. make up bootstraps clients, humans, and machine keys into e2e-ui.env.',
			where: 'docker/ · scripts/up.sh · GET /signin',
			href: '/signin?callbackUrl=/todos',
			label: 'Sign in'
		}
	];

	// —— CQRS samples (truncated from real fixture crates) ——
	const sampleAggregate = `// crates/todo-domain — aggregate owns invariants
#[sourced(entity, events = "TodoEvent", aggregate_type = "todo")]
impl Todo {
  pub fn create(
    &mut self,
    todo_id: impl Into<String>,
    owner_id: impl Into<String>, // auth user — not peer body
    title: impl Into<String>,
  ) -> Result<(), TodoError> {
    // … validate empty id/owner/title …
    self.record_created(todo_id, owner_id, title)?;
    Ok(())
  }

  #[event("todo.created")]
  fn record_created(&mut self, todo_id: String, owner_id: String, title: String) {
    self.todo_id = todo_id;
    self.owner_id = owner_id;
    self.title = title;
    self.status = TodoStatus::Open;
  }
}`;

	const sampleCommand = `// crates/service/handlers/commands/create.rs — todo.create
pub async fn handle(ctx: &Context<'_, TodoDeps<…>>) -> Result<Value, HandlerError> {
  let owner = require_user(ctx.session())?; // identity from session
  let input = ctx.input::<Input>()?;        // todo_id, title

  let mut todo = Todo::default();
  todo.create(&input.todo_id, &owner, &input.title)?;

  let fact = TodoFact::from_todo(&todo);
  let outbox = OutboxMessage::encode(…, "todo.created", &fact)?;
  ctx.repo().outbox(outbox).commit(&mut todo).await?;
  // commands never write the read model
  Ok(json!({ "todo_id": fact.todo_id, "status": fact.status }))
}`;

	const sampleReadModel = `// crates/readmodels — projected rows only
#[derive(ReadModel)]
#[table("todos")]
pub struct TodoView {
  #[id("todo_id")]
  pub todo_id: String,
  pub owner_id: String, // GraphQL filters: owner_id = claim(x-user-id)
  pub title: String,
  pub status: String,   // open | completed | archived
}

pub fn map_todo_fact(e: &TodoFact) -> TodoView {
  TodoView {
    todo_id: e.todo_id.clone(),
    owner_id: e.owner_id.clone(),
    title: e.title.clone(),
    status: e.status.clone(),
  }
}`;

	const sampleProjector = `// crates/service/handlers/events/project_todo.rs
// Project any todo.* fact → todos read model
pub const EVENTS: &[&str] = &[
  "todo.created", "todo.renamed", "todo.completed",
  "todo.reopened", "todo.archived",
];

pub async fn handle(ctx: &Context<'_, TodoDeps<…>>) -> Result<Value, HandlerError> {
  let fact: TodoFact = decode_payload(ctx.message())?;
  let row = map_fact(&fact);
  let mut plan = ReadModelWritePlanBuilder::new();
  plan.upsert(&row)?;
  plan.commit(ctx.read_model_store()).await?;
  Ok(json!({ "todo_id": fact.todo_id, "status": fact.status }))
}`;

	const sampleService = `// crates/service — one service, many hosts
pub fn build_service(repo, locks, read_models) -> Service {
  let todos = routes!(…, command create, …, events project_todo);
  let chat  = routes!(…, command chat_post, event project_chat);
  Service::new().named("e2e-ui").routes(todos).routes(chat)
}

// crates/runner — configure SQLite vs Postgres + identity
let database_url = env::var("DATABASE_URL")
  .unwrap_or_else(|_| "sqlite:./e2e-ui.db?mode=rwc".into());
let identity = identity_from_env(); // OidcBearer or DevHeaders

if database_url.starts_with("postgres") {
  run_postgres(&database_url, &bind, identity).await
} else {
  run_sqlite(&database_url, &bind, identity).await
}`;
</script>

<div class="wf-home">
	<section class="wf-hero">
		<div class="wf-hero-inner">
			<span class="wf-kicker">Distributed · e2e-ui template</span>
			<h1>
				A <em>framework template</em> you run as full e2e tests — kept honest by the library.
			</h1>
			<p class="wf-lede">
				Neutral shell for envisioning your product: multi-crate CQRS, GraphQL with row-level
				filters, WebSocket subscriptions, real OIDC. The same folder is the living suite —
				<code>make test</code> offline, <code>make test-live</code> against Postgres + Zitadel.
			</p>
			<div class="wf-actions">
				{#if signedIn}
					<a class="wf-btn wf-btn-primary" href="/todos">Open todos</a>
					<a class="wf-btn wf-btn-ghost" href="/chat">Lobby chat</a>
				{:else}
					<a class="wf-btn wf-btn-primary" href="/signin?callbackUrl=/todos">Sign in with OIDC</a>
					<a class="wf-btn wf-btn-ghost" href="#architecture">See architecture</a>
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

	<section class="wf-section" id="story">
		<div class="wf-section-head">
			<span class="wf-label">Why this exists</span>
			<h2>Template first. Product later.</h2>
			<p>
				Not a product marketing site — the UI face of a fixture that ships with Distributed. When
				patterns change, the suite and this app move with them. Use it as a wireframe for your own
				app’s structure.
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
	</section>

	<section class="wf-run" id="run">
		<div class="wf-run-inner">
			<div class="wf-section-head">
				<span class="wf-label">Run</span>
				<h2>Three commands. Full stack.</h2>
				<p>
					From <code>tests/e2e-ui</code>. Demo password after bootstrap:
					<code>Password1!</code> (alice / bob / admin).
				</p>
			</div>
			<div class="wf-steps">
				<div class="wf-step">
					<h3>Bootstrap IdP + DB</h3>
					<p><code>make up</code> writes <code>e2e-ui.env</code> with issuer, client, machine keys.</p>
				</div>
				<div class="wf-step">
					<h3>API + UI</h3>
					<p><code>make run</code> — GraphQL :8791, SvelteKit :5180, env loaded.</p>
				</div>
				<div class="wf-step">
					<h3>Prove it</h3>
					<p><code>make test</code> offline; <code>make test-live</code> for OIDC isolation.</p>
				</div>
			</div>
		</div>
	</section>

	<section class="wf-section" id="demos">
		<div class="wf-section-head">
			<span class="wf-label">Live demos</span>
			<h2>What is demonstrated — and where</h2>
			<p>
				Each capability maps to a UI route or crate path. Open after
				<code>make up && make run</code>.
			</p>
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
	</section>

	<section class="wf-section" id="architecture">
		<div class="wf-section-head">
			<span class="wf-label">Architecture · todos</span>
			<h2>One feature, five layers</h2>
			<p>
				How a todo flows through the fixture: pure aggregate → command handler → outbox fact →
				projector → read model. The service crate composes routes; the runner picks SQLite or
				Postgres and OIDC vs DevHeaders.
			</p>
		</div>

		<div class="wf-arch">
			<article class="wf-sample" data-sample="aggregate">
				<div class="wf-sample-meta">
					<h3>1 · Aggregate</h3>
					<p>
						Domain owns invariants. Commands never dual-write; events are recorded on the entity.
					</p>
					<span class="wf-sample-path">crates/todo-domain/src/lib.rs</span>
				</div>
				<div class="wf-code">
					<div class="wf-code-bar">
						<span>todo-domain</span>
						<em>Aggregate</em>
					</div>
					<pre><code>{sampleAggregate}</code></pre>
				</div>
			</article>

			<article class="wf-sample" data-sample="command-handler">
				<div class="wf-sample-meta">
					<h3>2 · Command handler</h3>
					<p>
						Microservice handler loads session identity, mutates the aggregate, commits with an
						outbox message. No read-model writes here.
					</p>
					<span class="wf-sample-path"
						>crates/service/src/handlers/commands/create.rs · todo.create</span
					>
				</div>
				<div class="wf-code">
					<div class="wf-code-bar">
						<span>handlers/commands/create.rs</span>
						<em>Command handler</em>
					</div>
					<pre><code>{sampleCommand}</code></pre>
				</div>
			</article>

			<article class="wf-sample" data-sample="read-model">
				<div class="wf-sample-meta">
					<h3>3 · Read model</h3>
					<p>
						Query-side table shape. GraphQL filters <code>owner_id</code> to the caller’s
						<code>x-user-id</code> claim for users.
					</p>
					<span class="wf-sample-path">crates/readmodels/src/lib.rs · TodoView</span>
				</div>
				<div class="wf-code">
					<div class="wf-code-bar">
						<span>e2e-readmodels</span>
						<em>Read model</em>
					</div>
					<pre><code>{sampleReadModel}</code></pre>
				</div>
			</article>

			<article class="wf-sample" data-sample="projection-handler">
				<div class="wf-sample-meta">
					<h3>4 · Projection handler</h3>
					<p>
						Event handlers are the only writers of read models. One projector covers all
						<code>todo.*</code> facts via upsert.
					</p>
					<span class="wf-sample-path"
						>crates/service/src/handlers/events/project_todo.rs</span
					>
				</div>
				<div class="wf-code">
					<div class="wf-code-bar">
						<span>handlers/events/project_todo.rs</span>
						<em>Projection handler</em>
					</div>
					<pre><code>{sampleProjector}</code></pre>
				</div>
			</article>

			<article class="wf-sample" data-sample="service-config">
				<div class="wf-sample-meta">
					<h3>5 · Service + runner modes</h3>
					<p>
						<code>build_service</code> wires command + event routes once. The runner chooses
						SQLite vs Postgres storage and OidcBearer vs DevHeaders from env.
					</p>
					<span class="wf-sample-path"
						>crates/service/src/service.rs · crates/runner/src/main.rs</span
					>
				</div>
				<div class="wf-code">
					<div class="wf-code-bar">
						<span>e2e-service + e2e-runner</span>
						<em>Multi-mode config</em>
					</div>
					<pre><code>{sampleService}</code></pre>
				</div>
			</article>
		</div>
	</section>

	<section class="wf-cta">
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
	</section>

	<Footer />
</div>
