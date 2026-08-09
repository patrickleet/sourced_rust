<script lang="ts">
	/**
	 * Distributed product home — need/story first, then how the framework delivers it.
	 */
	import '$lib/styles/home.css';
	import { page } from '$app/state';
	import Footer from '$lib/components/shared/Footer.svelte';
	import { highlightCode } from '$lib/components/walkthrough/highlight';

	const session = $derived(page.data.session);
	const signedIn = $derived(!!session?.user);
	const authConfigError = $derived(page.url.searchParams.get('error') === 'Configuration');

	const toc = [
		{ href: '#claim', label: 'The claim' },
		{ href: '#sota-backend', label: 'SOTA backend' },
		{ href: '#sota-rust', label: 'Why Rust' },
		{ href: '#sota-frontend', label: 'SOTA frontend' },
		{ href: '#sota-together', label: 'One vehicle' },
		{ href: '#author', label: 'Backstory' },
		{ href: '#flow', label: 'How' },
		{ href: '#cqrs', label: 'CQRS' },
		{ href: '#aggregates', label: 'Aggregates' },
		{ href: '#read-models', label: 'Read models' },
		{ href: '#projections', label: 'Projections' },
		{ href: '#replica', label: 'Replica' },
		{ href: '#try', label: 'Playground' }
	];

	const demos = [
		{ href: '/chat', title: 'Lobby chat', tag: 'Live + anonymous', blurb: 'A shared room with SSR, live updates, and guest reads.' },
		{ href: '/todos', title: 'Todos', tag: 'Eventual', blurb: 'Ownership rules, optimistic commands, projector fill.' },
		{ href: '/blob', tag: 'Atomic', title: 'Blob game', blurb: 'Game moves with an atomic board in the response.' },
		{ href: '/admin', title: 'Admin', tag: 'Surface', blurb: 'Elevated surface — separate client, more power.' },
		{ href: '/session', title: 'Session', tag: 'OIDC', blurb: 'Who you are to the app: tokens, groups, roles.' }
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
projection! {
    pub const TODOS: ProjectionDescriptor<EventualOnly> = {
        name: "project_todos",
        version: 1,
        model: Todos,
        on {
            events: [
                TodoCreatedDomainEvent,
                TodoCompletedDomainEvent,
                TodoArchivedDomainEvent,
                // …
            ],
            mutation: save_todo,
            input: { todo: body },
        },
        on {
            events: [TodoPurgedDomainEvent],
            mutation: delete_todo,
            input: { todo_id: aggregate_id },
        },
    };
}`;

	const codeMutationGql = `# Syntax-only IR → MutationProgram (not a public GraphQL field).
# Same program applies to the SQL read model and the browser replica.
# Clients never call this — they send domain commands.
mutation SaveTodo {
  upsert_Todos(object: $input.todo)
}`;

	const codeCommand = `// Handler: load aggregate → domain method → commit events
pub async fn handle(
    ctx: &CausalCommandContext<'_, Todo>,
    input: TodoArchiveInput,
) -> Result<PreparedCommand<Eventual<TodoArchivePayload>>, HandlerError> {
    let owner = ctx.user_id()?.to_string();
    let mut todo = ctx.repo()
        .get(&input.todo_id).await?
        .ok_or_else(|| HandlerError::NotFound(input.todo_id.clone()))?;
    todo.archive(&owner).map_err(rejected)?;

    let state = TodoState::from(&*todo);
    ctx.repo().publish_events().commit(todo)?.eventual(TodoArchivePayload {
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

const query = Todos.use();
const commands = useCommands();
const todos = $derived($query.complete ? $query.data.todos : []);

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

			<span class="wf-kicker">End to end · Rust · TypeScript</span>
			<h1>
				<strong>Distributed</strong> is a
				<em>state-of-the-art</em> framework
				for building distributed systems, and realtime applications.
			</h1>
			<p class="wf-lede">
				Not a partial toolkit. An end-to-end cloud native stack — domain, service, query edge, live client, and even gitops —
				so engineers who care about quality code can stay on the model and still ship polished, fast,
				maintainable products.
			</p>

			<ul class="wf-hero-stack" aria-label="Stack">
				<li>Rust</li>
				<li>TypeScript</li>
				<li>CQRS / ES</li>
				<li>SvelteKit</li>
			</ul>

			<div class="wf-actions">
				{#if signedIn}
					<a class="wf-btn wf-btn-primary" href="/chat">Open the playground</a>
					<a class="wf-btn wf-btn-ghost" href="#claim">Prove it</a>
				{:else}
					<a class="wf-btn wf-btn-primary" href="/chat">Browse the lobby</a>
					<a class="wf-btn wf-btn-ghost" href="#claim">Prove it</a>
				{/if}
			</div>

			<p class="dist-hero-note">
				This site is the living playground — real apps under <code>tests/e2e-ui</code>.
			</p>

			<nav class="wf-toc" aria-label="On this page">
				{#each toc as item}
					<a href={item.href}>{item.label}</a>
				{/each}
			</nav>
		</div>
	</section>

	<section class="wf-band dist-claim-band" id="claim" aria-label="Open loop">
		<div class="wf-band-inner dist-claim-inner">
			<p class="dist-claim">
				“State of the art” is a strong claim… Here’s how we define it.
			</p>
			<div class="dist-claim-links">
				<a href="#sota-backend">Backend</a>
				<span aria-hidden="true">·</span>
				<a href="#sota-rust">Rust</a>
				<span aria-hidden="true">·</span>
				<a href="#sota-frontend">Front end</a>
				<span aria-hidden="true">·</span>
				<a href="#author">Backstory</a>
				<span aria-hidden="true">·</span>
				<a href="#flow">How it delivers</a>
			</div>
		</div>
	</section>

	<section class="wf-band wf-band-dark" id="sota-backend">
		<div class="wf-band-inner">
			<div class="wf-section-head wf-section-head-wide">
				<span class="wf-label">State of the art · Backend</span>
				<h2>Event-driven systems with room to scale</h2>
				<p>
					You never get perfect consistency, always-available writes, and surviving network partitions
					all at once — that is the <strong>CAP theorem</strong>. Under partition you must sacrifice
					one: usually consistency or availability. Most products that stay up choose availability
					and accept <strong>eventual consistency</strong> on the read side — with clear rules about
					what the user can trust immediately.
				</p>
			</div>

			<div class="dist-teach">
				<div class="dist-teach-block">
					<h3>Unidirectional flow and event-driven architecture</h3>
					<p>
						These go hand in hand: <strong>events drive the flow</strong>. Changes move in one
						direction, with order — command in, domain event out, projections update reads. The UI
						does not patch database tables; it asks the system to do something, the system records
						facts, and views update from those facts. Front-end developers already know a cousin of
						this from Redux-style stores: dispatch → reduce → select. State-of-the-art backends
						apply the same discipline <strong>across services</strong>, not only inside one SPA —
						with events as the spine that keeps order honest.
					</p>
					<p>
						Contrast the “microservices” setup you have probably seen: a web of services calling
						each other over HTTP whenever convenient — A calls B, B calls C, C calls A under load,
						timeouts cascade, and nobody can explain the order of effects. That is not a
						distributed system; it is distributed chaos. Event-driven, unidirectional design
						replaces request spaghetti with a path you can reason about.
					</p>
				</div>
				<div class="dist-teach-block">
					<h3>CQRS and event sourcing</h3>
					<p>
						<strong>Command query responsibility segregation</strong> separates the model that is
						best for enforcing business rules from the model that is best for answering screens.
						Commands load <strong>aggregates</strong> (transactional consistency boundaries).
						Queries hit a <strong>read model</strong> optimized for lists, joins, and filters.
						<strong>Event sourcing</strong> means the write side records what happened as an
						append-only history — plain structs / POJOs and methods you can unit-test — instead of
						only the latest row overwrite.
					</p>
				</div>
				<div class="dist-teach-block">
					<h3>Domain events and projections</h3>
					<p>
						When an aggregate accepts a command, it emits <strong>domain events</strong>: immutable
						facts about the change. <strong>Projections</strong> subscribe to those facts and update
						SQL (or other) read models. That is how you keep “what the business decided” separate
						from “what the UI needs to list,” without dual-writing from the client.
					</p>
				</div>
				<div class="dist-teach-block">
					<h3>Identity and authorization</h3>
					<p>
						Real products need real identity: <strong>OIDC</strong>, sessions, and
						<strong>JWTs</strong> that carry claims. Authorization belongs next to the data —
						<strong>RBAC</strong> on rows and columns of the read model, and the same actor claims
						on commands — not a one-off middleware story per endpoint.
					</p>
				</div>
			</div>
			<p class="dist-teach-foot">
				That is the backend half of the bar: event-driven, unidirectional, CQRS/ES, projections, honest
				consistency tradeoffs, and identity that the model can use. The runtime for that half has to
				earn its place for reasons beyond “it’s fast.”
			</p>
		</div>
	</section>

	<section class="wf-band wf-band-light" id="sota-rust">
		<div class="wf-band-inner">
			<div class="wf-section-head wf-section-head-wide">
				<span class="wf-label">Why Rust</span>
				<h2>Performance and safety without giving up a clean domain API</h2>
				<p>
					State-of-the-art distributed backends need predictable performance under concurrency, hard
					edges around memory and data races, and a type system that catches mistakes before they
					ship. Rust was built for that class of problem — systems work without the usual crash
					surface of unmanaged languages, and without the GC pauses that make latency SLOs harder.
				</p>
			</div>

			<div class="dist-teach">
				<div class="dist-teach-block">
					<h3>Memory safety without a garbage collector</h3>
					<p>
						Ownership and borrowing enforce “who can touch this data” at compile time. You get
						C-class control of layout and lifetime, with whole categories of use-after-free and
						data-race bugs ruled out before the binary runs. For services that process events and
						commands all day, that is not academic — it is fewer production landmines and more
						stable latency under load.
					</p>
				</div>
				<div class="dist-teach-block">
					<h3>Fearless concurrency</h3>
					<p>
						Async runtimes (Tokio and friends), channels, and the type system’s
						<code>Send</code> / <code>Sync</code> bounds make concurrent work explicit. You can
						share infrastructure across cores without pretending shared mutable state is fine by
						default. That matches how real backends actually run: many in-flight commands, many
						projections, many connections.
					</p>
				</div>
				<div class="dist-teach-block">
					<h3>A type system that encodes the domain</h3>
					<p>
						Result types, enums, and strong typing make invalid states harder to represent. Domain
						errors can be first-class; “this command was rejected” is not a random string thrown
						from a helper three layers down. When your product is business rules, the language
						should make those rules hard to mis-wire.
					</p>
				</div>
				<div class="dist-teach-block">
					<h3>Zero-cost abstractions — and macros as DX</h3>
					<p>
						Rust’s ethos is: high-level tools that compile down to efficient machine code. Macros
						and derives are how frameworks give you a simple authoring surface without a heavy
						runtime reflection tax. You write something that reads like the domain; the compiler
						emits the boilerplate for events, serialization, and wiring — so “record this domain
						event” or “on these events, update the read model” can stay short at the call site while
						the machinery still owns fingerprints, history, and lowerings.
					</p>
					<p>
						The same idea scales past one language: domain definitions can drive the client too —
						typed commands, replica metadata, how events map to cache updates — so front-end DX is
						generated alignment, not a second hand-written stack. Compilers (and codegen) are the
						framework, end to end.
					</p>
				</div>
			</div>
			<p class="dist-teach-foot">
				So yes — Rust, for this class of work. Not as a fashion statement: as a substrate that lets a
				CQRS/ES stack be both strict and pleasant to author, and a credible source of truth for the
				client.
			</p>
		</div>
	</section>

	<section class="wf-band wf-band-dark" id="sota-frontend">
		<div class="wf-band-inner">
			<div class="wf-section-head wf-section-head-wide">
				<span class="wf-label">State of the art · Front end</span>
				<h2>Compilers are the new frameworks</h2>
				<p>
					Modern client stacks win when they <em>compile away</em> glue: routing, data loading, type
					safety, and update paths. A state-of-the-art app path is not “fetch in
					<code>onMount</code> and hope.” It is server-rendered HTML, a rehydrated client that shares
					the same data contract, live updates without polling, and <strong>optimistic UI</strong>
					that stays honest with an eventually consistent backend.
				</p>
			</div>

			<div class="dist-teach">
				<div class="dist-teach-block">
					<h3>SvelteKit and the compiler mindset</h3>
					<p>
						Svelte and SvelteKit push work into the compiler and into load/live primitives so
						application code stays about the product. The same spirit applies to a full-stack
						system: generate the client from the server’s truth instead of hand-maintaining a second
						API and cache story.
					</p>
				</div>
				<div class="dist-teach-block">
					<h3>GraphQL as selection, not the product</h3>
					<p>
						<strong>GraphQL</strong> shines when the read model is already well defined: the page
						declares the fields it needs. You avoid a REST endpoint per screen. Writes still go
						through domain <strong>commands</strong> — not table CRUD exposed as “mutations” for the
						browser to call.
					</p>
				</div>
				<div class="dist-teach-block">
					<h3>Replica cache and optimistic updates</h3>
					<p>
						A <strong>client replica</strong> holds the authorized slice of the read model the UI
						cares about. When the user fires a command, state-of-the-art UX applies the expected
						result immediately (optimistic update), then converges when the projection lands. That
						only works cleanly if the same definitions that update the server read model also drive
						the client — not a one-off <code>setState</code> recipe per page.
					</p>
				</div>
				<div class="dist-teach-block">
					<h3>SSR, rehydration, and live</h3>
					<p>
						First paint from the server, same query rehydrated on the client, then a live feed for
						rooms and dashboards. One operation for seed, hydrate, and subscription beats
						maintaining a separate GraphQL query and WebSocket document that always drift apart.
					</p>
				</div>
			</div>
			<p class="dist-teach-foot">
				That is the frontend half of the bar: compiler-owned glue, selection APIs, a replica aligned
				with the domain, optimistic UI, and real-time without polling theater.
			</p>
		</div>
	</section>

	<section class="wf-band wf-band-light" id="sota-together">
		<div class="wf-band-inner">
			<div class="wf-section-head wf-section-head-wide">
				<span class="wf-label">The bar, whole</span>
				<h2>Two halves used to be two shopping lists</h2>
				<p>
					Brokers, projectors, Hasura-class query layers, a UI framework, OIDC, custom optimistic
					cache code — each can be excellent alone. The tax is integration: versions, auth boundaries,
					and two stories for “what a command does to the UI.” State of the art is not collecting
					best-in-class parts; it is one path from domain event to optimistic row that teams do not
					reinvent every time.
				</p>
				<p>
					<strong>Distributed</strong> is built to be that path — one coherent system so code
					generation can keep the developer experience simple. How we got here, then how it delivers.
				</p>
			</div>
			<div class="wf-actions dist-vehicle-actions">
				<a class="wf-btn wf-btn-primary" href="#author">How this project got here</a>
				<a class="wf-btn wf-btn-ghost" href="#flow">How it delivers</a>
			</div>
		</div>
	</section>

	<section class="wf-band wf-band-dark" id="author">
		<div class="wf-band-inner">
			<div class="wf-section-head wf-section-head-wide">
				<span class="wf-label">Backstory</span>
				<h2>Built by someone who’s lived the glue</h2>
				<p>
					I’m a multi-time CTO and long-time consultant on microservices and DevOps. Articles I’ve
					written have been read over a million times; thousands of people took free DevOps classes I
					ran. I’ve maintained <strong>sourced</strong> and <strong>servicebus</strong> in the Node
					ecosystem for nearly a decade, and I’ve been a student of domain-driven design since before
					CQRS/ES was the usual name for the write path — I was in the Google group when the name
					shifted, went to the conferences, and have helped teams from startups through enterprises
					actually ship this style of system.
				</p>
				<p>
					Early on I wired pieces together. <strong>Matt Walters</strong> was a mentor who taught me
					a great deal; he authored Node <code>sourced</code> and <code>servicebus</code>, which
					inspired parts of what this library does. Later, Knative Eventing replaced much of
					hand-rolled service-bus plumbing — messaging became more declarative, and language choice
					less tied to one library. For reads I used Hasura-style SQL: joins, RBAC, generated query
					APIs, opinionated for CQRS. It worked. It was still a kit of parts.
				</p>
				<p>
					I started <strong>Distributed</strong> in late 2024 (first commits October 2024) as AI
					became usable for real systems work — and as models got good enough that building the dream
					framework (everything in one coherent place) stopped being a multi-year solo fantasy. This
					playground is that system: domain through live UI, with the DX we always wanted.
				</p>
			</div>
			<div class="wf-actions dist-vehicle-actions">
				<a class="wf-btn wf-btn-primary" href="#flow">How the framework delivers</a>
				<a class="wf-btn wf-btn-ghost" href="#try">See the demos</a>
			</div>
		</div>
	</section>

	<section class="wf-band wf-band-light" id="flow">
		<div class="wf-band-inner">
			<div class="wf-section-head wf-section-head-wide dist-how-head">
				<span class="wf-label">How Distributed provides it</span>
				<h2>One unidirectional loop — end to end</h2>
				<p>
					That’s the bar. Here’s the magic path that closes it: codegen across the stack, one ordered
					cycle, each section below a stage with real code from this playground.
				</p>
			</div>
			<article class="wf-story-step dist-flow-step">
				<div class="wf-story-copy">
					<span class="wf-label">01 · Unidirectional</span>
					<h2 class="wf-step-title">Changes go one way. There is order.</h2>
					<p class="wf-why">
						Front-end developers know this from Redux: dispatch in, state updates on a defined path,
						UI reads the result. Distributed is that idea for the <strong>whole system</strong>.
					</p>
					<p class="wf-why">
						Client → <strong>command</strong> → <strong>aggregate</strong> state change →
						<strong>domain event</strong> → <strong>projection</strong> → <strong>read model</strong>
						→ client. No dual-write from the UI. CAP and eventual consistency sit on the read side;
						optimistic UI is how the front end meets that honestly.
					</p>
				</div>
				<figure
					class="dist-flow-diagram"
					aria-label="Unidirectional system flow: client through GraphQL gateway, then clockwise commands, aggregate, domain event, projection, read model, back to gateway"
				>
					<div class="dist-flow-circle">
						<!--
							viewBox 360×420 · ring center (180, 220) r=118
							Nodes on the ring · short gap arcs (pad ±18°)
						-->
						<svg class="dist-flow-circle-svg" viewBox="0 0 360 420" aria-hidden="true">
							<defs>
								<marker
									id="dist-flow-arrow"
									viewBox="0 0 12 12"
									refX="10"
									refY="6"
									markerWidth="7"
									markerHeight="7"
									orient="auto"
									markerUnits="userSpaceOnUse"
								>
									<path d="M1 1.5 L11 6 L1 10.5 Z" class="dist-flow-arrowhead" />
								</marker>
							</defs>

							<!-- Client → gateway -->
							<path
								class="dist-flow-connector"
								d="M 180 48 L 180 84"
								marker-end="url(#dist-flow-arrow)"
							/>

							<!-- Gateway → Commands (18°→42°) -->
							<path
								class="dist-flow-connector"
								d="M 216.46 107.79 A 118 118 0 0 1 258.99 132.33"
								marker-end="url(#dist-flow-arrow)"
							/>
							<!-- Commands → Aggregate (78°→102°) -->
							<path
								class="dist-flow-connector"
								d="M 295.41 195.45 A 118 118 0 0 1 295.41 244.55"
								marker-end="url(#dist-flow-arrow)"
							/>
							<!-- Aggregate → Domain event (138°→162°) -->
							<path
								class="dist-flow-connector"
								d="M 258.99 307.67 A 118 118 0 0 1 216.46 332.21"
								marker-end="url(#dist-flow-arrow)"
							/>
							<!-- Domain event → Projection (198°→222°) -->
							<path
								class="dist-flow-connector"
								d="M 143.54 332.21 A 118 118 0 0 1 101.01 307.67"
								marker-end="url(#dist-flow-arrow)"
							/>
							<!-- Projection → Read model (258°→282°) -->
							<path
								class="dist-flow-connector"
								d="M 64.59 244.55 A 118 118 0 0 1 64.59 195.45"
								marker-end="url(#dist-flow-arrow)"
							/>
							<!-- Read model → Gateway (318°→342°) -->
							<path
								class="dist-flow-connector"
								d="M 101.01 132.33 A 118 118 0 0 1 143.54 107.79"
								marker-end="url(#dist-flow-arrow)"
							/>
						</svg>

						<span class="dist-flow-orbit dist-flow-orbit-client">
							<span class="dist-flow-chip dist-flow-chip-client">Client</span>
						</span>
						<span class="dist-flow-entry-meta" aria-hidden="true">query · command · live</span>
						<span class="dist-flow-orbit dist-flow-orbit-0">
							<span class="dist-flow-chip dist-flow-chip-hub">GraphQL gateway</span>
						</span>
						<span class="dist-flow-orbit dist-flow-orbit-1">
							<span class="dist-flow-chip">Commands</span>
						</span>
						<span class="dist-flow-orbit dist-flow-orbit-2">
							<span class="dist-flow-chip">Aggregate</span>
						</span>
						<span class="dist-flow-orbit dist-flow-orbit-3">
							<span class="dist-flow-chip">Domain event</span>
						</span>
						<span class="dist-flow-orbit dist-flow-orbit-4">
							<span class="dist-flow-chip">Projection</span>
						</span>
						<span class="dist-flow-orbit dist-flow-orbit-5">
							<span class="dist-flow-chip dist-flow-chip-rm">Read model</span>
						</span>

						<span class="dist-flow-circle-core" aria-hidden="true">
							<span class="dist-flow-core-label">one way</span>
						</span>
					</div>
				</figure>
			</article>
		</div>
	</section>

	<section class="wf-band wf-band-dark" id="cqrs">
		<div class="wf-band-inner">
			<article class="wf-story-step">
				<div class="wf-story-copy">
					<span class="wf-label">02 · CQRS</span>
					<h2 class="wf-step-title">Decisions and views are different models</h2>
					<p class="wf-why">
						In the business, “complete this todo” is a decision with rules. “Show my open todos”
						is a question about a list. CQRS keeps those as separate models: commands load
						aggregates; queries hit a SQL-shaped read model. You avoid forcing both into “update a
						row,” so domain code stays about rules and screens stay about presentation — and both
						get simpler.
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
					<h2 class="wf-step-title">Business rules as plain types — with history</h2>
					<p class="wf-why">
						Express the business as ordinary Rust structs and methods: who may do what, what state
						is allowed next. Under the hood that’s event sourcing — repository, append-only
						events, optional upcasters — so you get a timeline and easy unit tests without putting
						rules in SQL or HTTP. The domain stays the place where expertise lives.
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
					<h2 class="wf-step-title">What the user is allowed to see</h2>
					<p class="wf-why">
						Screens need tables: lists, filters, joins. Read models are that query shape, with
						row/column permissions next to the model — “owner sees only their todos,” “admin sees
						all.” SQL does what it’s good at; auth is not a second story bolted on later. Queries
						and commands share the same idea of who the actor is.
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
					<h2 class="wf-step-title">Page needs → typed client</h2>
					<p class="wf-why">
						Once the read model is defined, GraphQL is how the UI selects fields — transport for
						queries, not the heart of the product. You declare what a page needs; you don’t maintain
						a REST endpoint per screen. Commands stay domain verbs on the write side.
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
					<span class="wf-label">06 · Projections</span>
					<h2 class="wf-step-title">When the domain changes, views catch up</h2>
					<p class="wf-why">
						Next step in the unidirectional cycle: after a command succeeds, events describe what
						happened. Projections turn those facts into read-model updates — and the same mapping
						drives optimistic UI in the browser. You declare “on these events, update the view like
						this” once; you don’t dual-write tables from the page or invent a second cache language.
					</p>
					<p class="wf-why">
						The mutation file looks like GraphQL but is internal IR for that update program, not a
						public client mutation API. Pages still send domain commands.
					</p>
					<span class="wf-sample-path"
						>tests/e2e-ui/crates/projections/src/todos.rs · mutations/save_todo.mutation.graphql</span
					>
				</div>
				<div class="wf-code-stack">
					<div class="wf-code">
						<div class="wf-code-bar">
							<span>todos.rs</span>
							<em>projection</em>
						</div>
						<pre><code>{@html highlightCode(codeProjection)}</code></pre>
					</div>
					<div class="wf-code">
						<div class="wf-code-bar">
							<span>save_todo.mutation.graphql</span>
							<em>mutation ir</em>
						</div>
						<pre><code>{@html highlightCode(codeMutationGql)}</code></pre>
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
					<h2 class="wf-step-title">The cycle closes at the client</h2>
					<p class="wf-why">
						Generated TypeScript carries your inventory into a client replica and typed commands.
						The page reads <code>query.use()</code> and calls <code>commands.todo…</code> — same
						business verbs as the server. Optimism applies the projection mapping on the way around
						the loop, so the UI stays aligned with the model instead of a one-off cache recipe per
						screen.
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

	<section class="wf-band wf-band-dark" id="sveltekit">
		<div class="wf-band-inner">
			<article class="wf-story-step">
				<div class="wf-story-copy">
					<span class="wf-label">08 · SvelteKit</span>
					<h2 class="wf-step-title">SSR first, then live — one query</h2>
					<p class="wf-why">
						<code>@load</code> and <code>@live</code> use the same GraphQL operation for server
						render, rehydrate, and a push change feed. Users get a fast first paint and rooms that
						stay current without you maintaining a second subscription document or polling.
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

	<section class="wf-band wf-band-light" id="oidc">
		<div class="wf-band-inner">
			<article class="wf-story-step">
				<div class="wf-story-copy">
					<span class="wf-label">09 · OIDC</span>
					<h2 class="wf-step-title">Who the user is — in the model and the UI</h2>
					<p class="wf-why">
						Real products need real identity. OIDC is first-class (Zitadel here; Keycloak and
						Authentik in tests). Sessions and JWTs become claims the domain already uses for
						ownership and roles — the same claims that scope the client replica.
					</p>
				</div>
				<div class="dist-pillars">
					<article class="dist-pillar">
						<h3>Claims → RBAC</h3>
						<p>Row filters and command handlers share claims like <code>x-user-id</code> and roles.</p>
					</article>
					<article class="dist-pillar">
						<h3>Surfaces</h3>
						<p>User, admin, and public clients stay separate so elevated power does not leak.</p>
					</article>
					<article class="dist-pillar">
						<h3>Try it</h3>
						<p>
							<a class="dist-inline-link" href="/session">Session</a> ·
							<a class="dist-inline-link" href="/signin?callbackUrl=/todos">sign in</a>
							(alice / bob / admin)
						</p>
					</article>
				</div>
			</article>
		</div>
	</section>

	<section class="wf-band wf-band-dark" id="try">
		<div class="wf-band-inner">
			<div class="wf-section-head">
				<span class="wf-label">Playground</span>
				<h2>Small apps, full patterns</h2>
				<p>
					Real features under <code>tests/e2e-ui</code> — chat, todos, a game, admin. Each has
					<strong>How it’s built</strong>: from the page down to domain and events.
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
			<p class="dist-run-hint dist-run-hint-light">
				Demo logins after <code>make up</code>: <code>alice</code> / <code>bob</code> /
				<code>admin</code> · <code>Password1!</code>
			</p>
		</div>
	</section>

	<section class="wf-band wf-band-dark dist-closing">
		<div class="wf-band-inner">
			<div class="wf-section-head">
				<span class="wf-label">Next</span>
				<h2>Model the domain. Leave the rest to the stack.</h2>
				<p>
					Pick a demo close to your problem — todos for ownership and rules, chat for live rooms —
					and reuse the shapes. The vehicle is one framework, built to scale; you stay on the parts
					that create customer value. Fleet hosting (<strong>ops.com.ai</strong>) is on the roadmap.
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
