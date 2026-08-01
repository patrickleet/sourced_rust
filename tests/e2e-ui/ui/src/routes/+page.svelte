<script lang="ts">
	/**
	 * Distributed product home — canon framing from project owner (2026-08-01).
	 * Features follow CQRS → ES → SQL/RBAC → projections → replica → SvelteKit → OIDC.
	 * GraphQL is transport/selection, not the product identity.
	 */
	import '$lib/styles/home.css';
	import { page } from '$app/state';
	import Footer from '$lib/components/shared/Footer.svelte';
	import { highlightCode } from '$lib/components/walkthrough/highlight';

	const session = $derived(page.data.session);
	const signedIn = $derived(!!session?.user);
	const authConfigError = $derived(page.url.searchParams.get('error') === 'Configuration');

	const toc = [
		{ href: '#foundations', label: 'Foundations' },
		{ href: '#cqrs', label: 'CQRS' },
		{ href: '#aggregates', label: 'Aggregates' },
		{ href: '#read-models', label: 'Read models' },
		{ href: '#query-api', label: 'Query API' },
		{ href: '#projections', label: 'Projections' },
		{ href: '#replica', label: 'Replica' },
		{ href: '#pages', label: 'Pages' },
		{ href: '#sveltekit', label: 'SvelteKit' },
		{ href: '#oidc', label: 'OIDC' },
		{ href: '#try', label: 'Playground' }
	];

	const demos = [
		{ href: '/chat', title: 'Lobby chat', tag: 'Live + anonymous', blurb: 'SSR, @live, and guest reads on a public surface.' },
		{ href: '/todos', title: 'Todos', tag: 'Causal', blurb: 'Owner RLS, modeled optimism, projector fill.' },
		{ href: '/blob', tag: 'Projected', title: 'Blob game', blurb: 'Atomic board in the mutation payload.' },
		{ href: '/admin', title: 'Admin', tag: 'Surface', blurb: 'Second client for elevated ops.' },
		{ href: '/session', title: 'Session', tag: 'OIDC', blurb: 'Tokens, groups, and engine roles.' },
		{ href: '/public', title: 'Public', tag: 'Anonymous', blurb: 'Empty identity + named surface contract.' }
	];

	// —— Code samples from the living playground (trimmed for teaching) ——
	const codeDomain = `#[sourced(
    entity,
    events = "TodoEvent",
    aggregate_type = "todo",
    domain_state = TodoState,
)]
impl Todo {
    pub fn create(
        &mut self,
        todo_id: impl Into<String>,
        owner_id: impl Into<String>,
        title: impl Into<String>,
    ) -> Result<(), TodoError> {
        // …validate…
        self.record_created(todo_id, owner_id, title)?;
        Ok(())
    }

    #[event("todo.created", version = 1, domain)]
    fn record_created(&mut self, todo_id: String, owner_id: String, title: String) {
        self.entity.set_id(&todo_id);
        self.todo_id = todo_id;
        self.owner_id = owner_id;
        self.title = title;
        self.status = TodoStatus::Open;
    }
}`;

	const codeReadModel = `#[derive(Clone, Debug, ReadModel)]
#[readmodel(primary_key = ["todo_id"])]
pub struct Todos {
    #[readmodel(id)]
    pub todo_id: String,
    pub owner_id: String,
    pub title: String,
    pub status: String,
}

impl Todos {
    pub fn permissions() -> ModelPermissions<Self> {
        ModelPermissions::new()
            .grant(
                "user",
                read()
                    .all_columns()
                    .rows(col("owner_id").eq(claim("x-user-id"))),
            )
            .grant("admin", read().all_columns())
    }
}`;

	const codeProjection = `// Event → mutation mapping (server projector + client optimism)
portable_handlers! {
    pub const TODOS: ProjectionDescriptor<EventualOnly> = {
        name: "project_todos",
        version: 1,
        model: Todos,
        on_event {
            TodoCreatedDomainEvent,
            TodoCompletedDomainEvent,
            TodoArchivedDomainEvent,
            // …
        }
        apply save_todo as "todo",
        on_deleted TodoPurgedDomainEvent => apply delete_todo as "todo_id",
    };
}

// save_todo.mutation.graphql (syntax-only IR, not a public GQL field)
// mutation SaveTodo { upsert_Todos(object: $input.todo) }`;

	const codeCommand = `// Handler: load aggregate → domain method → commit events
pub async fn handle(
    ctx: &CausalCommandContext<'_, Todo>,
    input: TodoArchiveInput,
) -> Result<PreparedCommand<Causal<TodoArchivePayload>>, HandlerError> {
    let owner = ctx.user_id()?.to_string();
    let mut todo = ctx.repo()
        .get(&input.todo_id).await?
        .ok_or_else(|| HandlerError::NotFound(input.todo_id.clone()))?;
    todo.archive(&owner).map_err(rejected)?;

    let state = TodoState::from(&*todo);
    ctx.repo().publish_events().commit(todo)?.causal(TodoArchivePayload {
        todo_id: state.todo_id,
        status: state.status,
    })
}`;

	const codePageQuery = `# Page declares the shape it needs — no hand-written query API
query Todos @load {
  todos(order_by: [{ status: asc }, { todo_id: asc }]) {
    todo_id
    owner_id
    title
    status
  }
}`;

	const codeUi = `// Generated operation + typed commands — no cache recipes in the page
import { Todos, useCommands } from '$distributed';

const list = Todos.use();
const commands = useCommands();
const rows = $derived($list.complete ? $list.data.todos : []);

await commands.todo.create({ title: text });
await commands.todo.complete({ todo_id });`;

	const codeLive = `# Same query powers SSR (@load) and live change feed (@live)
query ChatMessages($limit: Int!, $offset: Int!) @load @live {
  chat_messages(
    where: { room_id: { _eq: "lobby" } }
    limit: $limit
    offset: $offset
    order_by: [{ created_at: desc }]
  ) {
    message_id
    body
    author { display_name }
  }
}`;

	const codeCqrs = `// Commands → aggregates (accept / reject business rules)
commands.todo.create({ title })
commands.todo.archive({ todo_id })

// Queries → SQL-shaped read models (never write tables)
query Todos @load {
  todos { todo_id title status }
}`;

	const codeFlow = `// Command path
Command  →  Aggregate (event-sourced)
              ↓ emit domain events
Service bus  →  Projectors
              ↓ mutations
Read model (SQL)  →  GraphQL query edge
              ↓ same mappings
Browser replica  →  optimistic UI`;
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

			<span class="wf-kicker">Cloud native · Rust · TypeScript</span>
			<h1>
				Simple realtime apps on
				<em>distributed systems foundations</em>.
			</h1>
			<p class="wf-lede">
				<strong>Distributed</strong> is a cloud-native Rust and TypeScript framework for building
				simple, realtime, performant, and scalable applications based on distributed systems
				programming foundations — domain-driven design, CQRS, and event sourcing.
			</p>
			<p class="wf-lede dist-lede-follow">
				Made to start simple, and scale big. Run as a single backend service with a UI, or run the
				same models and handlers as microservices.
			</p>

			<ul class="wf-hero-stack" aria-label="Stack">
				<li>CQRS</li>
				<li>Event sourcing</li>
				<li>SQL read models</li>
				<li>Realtime</li>
			</ul>

			<div class="wf-actions">
				{#if signedIn}
					<a class="wf-btn wf-btn-primary" href="/chat">Open the playground</a>
					<a class="wf-btn wf-btn-ghost" href="#features">How it works</a>
				{:else}
					<a class="wf-btn wf-btn-primary" href="/chat">Browse the lobby</a>
					<a class="wf-btn wf-btn-ghost" href="#features">How it works</a>
				{/if}
			</div>

			<p class="dist-hero-note">
				This site is the official <strong>living playground</strong> for Distributed. Open any demo
				and use <em>How it’s built</em> for the full code walkthrough.
			</p>

			<nav class="wf-toc" aria-label="On this page">
				{#each toc as item}
					<a href={item.href}>{item.label}</a>
				{/each}
			</nav>
		</div>
	</section>

	<!-- Features: canon product narrative -->
	<section class="wf-band wf-band-dark" id="features">
		<div class="wf-band-inner">
			<div class="wf-section-head wf-section-head-wide">
				<span class="wf-label">Features</span>
				<h2>What serves you</h2>
				<p>
					We built decades of distributed-systems practice into the framework so you don’t have to
					reinvent it — but you should understand what each piece is <em>for</em>. The models that
					are best for changing aggregates and the models that are best for querying don’t line up.
					Distributed keeps them separate on purpose.
				</p>
			</div>
		</div>
	</section>

	<section class="wf-band wf-band-light" id="foundations">
		<div class="wf-band-inner">
			<article class="wf-story-step">
				<div class="wf-story-copy">
					<span class="wf-label">01 · Foundations</span>
					<h2 class="wf-step-title">DDD, CQRS, and event sourcing — built in</h2>
					<p class="wf-why">
						Domain-driven design, command/query separation, and event sourcing on aggregate roots
						are the substrate. You capture domain expertise in plain code; the framework owns
						history, publication, projections, and client wiring.
					</p>
				</div>
				<div class="wf-code-stack">
					<div class="wf-code">
						<div class="wf-code-bar">
							<span>mental model</span>
							<em>flow</em>
						</div>
						<pre><code>{@html highlightCode(codeFlow)}</code></pre>
					</div>
				</div>
			</article>
		</div>
	</section>

	<section class="wf-band wf-band-dark" id="cqrs">
		<div class="wf-band-inner">
			<article class="wf-story-step">
				<div class="wf-story-copy">
					<span class="wf-label">02 · CQRS</span>
					<h2 class="wf-step-title">Two models, on purpose</h2>
					<p class="wf-why">
						ORMs and Active Record often collapse “write the domain” into “update rows.” That’s fine
						for a lot of apps. When the logic matters — changeable, maintainable, full of real
						expertise — separate the shapes: a <strong>command model</strong> for aggregates, and a
						<strong>query model</strong> for reads. Command query responsibility segregation is the
						stance, not an optional diagram.
					</p>
				</div>
				<div class="wf-code-stack">
					<div class="wf-code">
						<div class="wf-code-bar">
							<span>commands vs queries</span>
							<em>api shape</em>
						</div>
						<pre><code>{@html highlightCode(codeCqrs)}</code></pre>
					</div>
				</div>
			</article>
		</div>
	</section>

	<section class="wf-band wf-band-light" id="aggregates">
		<div class="wf-band-inner">
			<article class="wf-story-step">
				<div class="wf-story-copy">
					<span class="wf-label">03 · Event-sourced aggregates</span>
					<h2 class="wf-step-title">Plain structs. History. Easy tests.</h2>
					<p class="wf-why">
						A strong model for changing aggregates is event sourcing: you work with plain Rust
						structures (or plain objects in any language), a repository, and append-only events.
						You get timeline history, and the part of the system that captures business expertise
						stays simple to read and unit-test — no HTTP or SQL required. (Event upcasters handle
						schema evolution when versions move.)
					</p>
					<span class="wf-sample-path">tests/e2e-ui/crates/todo-domain/src/models/todo.rs</span>
				</div>
				<div class="wf-code-stack">
					<div class="wf-code">
						<div class="wf-code-bar">
							<span>todo.rs</span>
							<em>domain</em>
						</div>
						<pre><code>{@html highlightCode(codeDomain)}</code></pre>
					</div>
				</div>
			</article>
		</div>
	</section>

	<section class="wf-band wf-band-dark" id="read-models">
		<div class="wf-band-inner">
			<article class="wf-story-step">
				<div class="wf-story-copy">
					<span class="wf-label">04 · SQL read models + RBAC</span>
					<h2 class="wf-step-title">SQL is great for querying</h2>
					<p class="wf-why">
						Tables, rows, columns, joins — and row/column RBAC on that shape. Define a normalized
						read model once. Permissions live next to the model, not as one-off middleware. The
						same role claims attach to commands.
					</p>
					<span class="wf-sample-path">tests/e2e-ui/crates/readmodels/src/models/todos.rs</span>
				</div>
				<div class="wf-code-stack">
					<div class="wf-code">
						<div class="wf-code-bar">
							<span>todos.rs</span>
							<em>read model</em>
						</div>
						<pre><code>{@html highlightCode(codeReadModel)}</code></pre>
					</div>
				</div>
			</article>
		</div>
	</section>

	<section class="wf-band wf-band-light" id="query-api">
		<div class="wf-band-inner">
			<article class="wf-story-step">
				<div class="wf-story-copy">
					<span class="wf-label">05 · Inferred query API</span>
					<h2 class="wf-step-title">Don’t hand-write query endpoints</h2>
					<p class="wf-why">
						If tables and relationships are defined well, we infer the query surface. GraphQL is
						how the frontend <em>selects</em> what a page needs — not the product identity. APIs are
						command calls and queries: commands load aggregates; queries hit the read model. You
						don’t maintain a parallel REST catalog for every list screen.
					</p>
					<span class="wf-sample-path">tests/e2e-ui/ui/src/routes/todos/+page.graphql</span>
				</div>
				<div class="wf-code-stack">
					<div class="wf-code">
						<div class="wf-code-bar">
							<span>+page.graphql</span>
							<em>selection</em>
						</div>
						<pre><code>{@html highlightCode(codePageQuery)}</code></pre>
					</div>
				</div>
			</article>
		</div>
	</section>

	<section class="wf-band wf-band-dark" id="projections">
		<div class="wf-band-inner">
			<article class="wf-story-step">
				<div class="wf-story-copy">
					<span class="wf-label">06 · Eventual consistency · bus · projections</span>
					<h2 class="wf-step-title">Immutable facts update the read model</h2>
					<p class="wf-why">
						CAP reality: large scalable systems are usually eventually consistent. Separate command
						and query models make that easy. Aggregates emit events (immutable facts). A
						<strong>service bus</strong> carries them. <strong>Projections</strong> map those events
						to read-model mutations — server-side when facts land, and compiled into client
						optimism so the UI doesn’t invent its own recipes.
					</p>
					<p class="wf-why">
						Event storming discovers aggregates and events; a long-term goal of the library is to
						turn those sessions (with AI) into a large fraction of the baseline wiring.
					</p>
					<span class="wf-sample-path">tests/e2e-ui/crates/projections/src/todos.rs</span>
				</div>
				<div class="wf-code-stack">
					<div class="wf-code">
						<div class="wf-code-bar">
							<span>todos.rs · save_todo.mutation.graphql</span>
							<em>projection</em>
						</div>
						<pre><code>{@html highlightCode(codeProjection)}</code></pre>
					</div>
					<div class="wf-code">
						<div class="wf-code-bar">
							<span>todo_archive.rs</span>
							<em>command handler</em>
						</div>
						<pre><code>{@html highlightCode(codeCommand)}</code></pre>
					</div>
				</div>
			</article>
		</div>
	</section>

	<section class="wf-band wf-band-light" id="replica">
		<div class="wf-band-inner">
			<article class="wf-story-step">
				<div class="wf-story-copy">
					<span class="wf-label">07 · Browser replica</span>
					<h2 class="wf-step-title">Optimistic UI without UI cache code</h2>
					<p class="wf-why">
						Because we own commands, events, projections, read models, and RBAC, we generate
						TypeScript that knows how commands and events map to mutations. The client keeps a
						<strong>replica</strong> of the authorized slice of data that matters for the app, and
						applies those mutations optimistically — based on Rust definitions, not one-off
						cache hacks in each page.
					</p>
					<span class="wf-sample-path">tests/e2e-ui/ui/src/routes/todos/+page.svelte</span>
				</div>
				<div class="wf-code-stack">
					<div class="wf-code">
						<div class="wf-code-bar">
							<span>+page.svelte</span>
							<em>ui</em>
						</div>
						<pre><code>{@html highlightCode(codeUi)}</code></pre>
					</div>
				</div>
			</article>
		</div>
	</section>

	<section class="wf-band wf-band-dark" id="pages">
		<div class="wf-band-inner">
			<article class="wf-story-step">
				<div class="wf-story-copy">
					<span class="wf-label">08 · Page data + typed commands</span>
					<h2 class="wf-step-title">Declare the shape; call the command</h2>
					<p class="wf-why">
						Generated TypeScript includes typed command functions. Each page (or operation file)
						declares the GraphQL selection for the data structure it needs. That’s the full
						contract for ordinary screens: selection + command, not dual APIs and manual cache
						policies.
					</p>
				</div>
				<div class="dist-pillars">
					<article class="dist-pillar">
						<h3>Selection</h3>
						<p>GraphQL operations name fields and arguments. The compiler owns normalization and indexes.</p>
					</article>
					<article class="dist-pillar">
						<h3>Commands</h3>
						<p><code>useCommands()</code> exposes domain verbs with typed inputs and causal results.</p>
					</article>
					<article class="dist-pillar">
						<h3>Honesty</h3>
						<p>Causal vs Projected semantics stay visible — eventual consistency without lying to the UI.</p>
					</article>
				</div>
			</article>
		</div>
	</section>

	<section class="wf-band wf-band-light" id="sveltekit">
		<div class="wf-band-inner">
			<article class="wf-story-step">
				<div class="wf-story-copy">
					<span class="wf-label">09 · SvelteKit</span>
					<h2 class="wf-step-title"><code>@load</code>, rehydration, and <code>@live</code></h2>
					<p class="wf-why">
						<code>@load</code> makes SSR automatic and rehydrates the client replica from the same
						operation. <code>@live</code> reuses that query as a WebSocket change feed powered by
						the <strong>change hub</strong> — push-driven updates instead of polling loops that
						guess freshness.
					</p>
					<span class="wf-sample-path">tests/e2e-ui/ui/src/routes/chat/+page.graphql</span>
				</div>
				<div class="wf-code-stack">
					<div class="wf-code">
						<div class="wf-code-bar">
							<span>+page.graphql</span>
							<em>load + live</em>
						</div>
						<pre><code>{@html highlightCode(codeLive)}</code></pre>
					</div>
				</div>
			</article>
		</div>
	</section>

	<section class="wf-band wf-band-dark" id="oidc">
		<div class="wf-band-inner">
			<article class="wf-story-step">
				<div class="wf-story-copy">
					<span class="wf-label">10 · Identity</span>
					<h2 class="wf-step-title">First-class OIDC</h2>
					<p class="wf-why">
						This playground uses <strong>Zitadel</strong>. Tests also prove
						<strong>Keycloak</strong> and <strong>Authentik</strong> (less fleshed out in demos).
						Sessions and JWTs supply claims — user id, roles, groups — that feed the same RBAC on
						queries and commands, and scope the client replica when identity changes.
					</p>
				</div>
				<div class="dist-pillars">
					<article class="dist-pillar">
						<h3>Claims → RBAC</h3>
						<p>Read models filter rows with claims like <code>x-user-id</code>; command handlers resolve the actor from context.</p>
					</article>
					<article class="dist-pillar">
						<h3>Surfaces</h3>
						<p>User, admin, and public clients are separate authorization-scoped inventories — not one open schema.</p>
					</article>
					<article class="dist-pillar">
						<h3>Try it</h3>
						<p>
							<a class="dist-inline-link" href="/session">Session</a> shows tokens and roles;
							<a class="dist-inline-link" href="/signin?callbackUrl=/todos">sign in</a> as alice / bob / admin.
						</p>
					</article>
				</div>
			</article>
		</div>
	</section>

	<section class="wf-band wf-band-light" id="try">
		<div class="wf-band-inner">
			<div class="wf-section-head">
				<span class="wf-label">Playground</span>
				<h2>Try it here</h2>
				<p>
					Real Distributed apps under <code>tests/e2e-ui</code>. Each route has a
					<strong>How it’s built</strong> panel — browser query, commands, handlers, domain, events,
					and RBAC.
				</p>
			</div>
			<div class="dist-demo-grid dist-demo-grid-light">
				{#each demos as d}
					<a class="dist-demo-card dist-demo-card-light" href={d.href}>
						<span class="dist-demo-tag dist-demo-tag-light">{d.tag}</span>
						<h3>{d.title}</h3>
						<p>{d.blurb}</p>
						<span class="dist-demo-go dist-demo-go-light">Open →</span>
					</a>
				{/each}
			</div>
		</div>
	</section>

	<section class="wf-band wf-band-dark" id="run">
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

	<section class="wf-band wf-band-light dist-closing">
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
