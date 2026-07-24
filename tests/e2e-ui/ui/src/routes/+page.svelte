<script lang="ts">
	/**
	 * Distributed framework template home.
	 * Living map of how e2e-ui actually works: OIDC → GraphQL RLS → replica cache →
	 * typed commands → aggregates → projectors (or same-tx RM for blob) → reads.
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
			title: 'Todos — your own field notes',
			blurb:
				'Create, complete, reopen, archive. The UI updates immediately; the server catches up a moment later. You only ever see your own notes unless you are admin.',
			where: '/todos · personal list · optimistic UI',
			label: 'Open todos'
		},
		{
			href: '/chat',
			title: 'Lobby chat — live with everyone',
			blurb:
				'A shared room that updates as people post. Names come from the identity directory, not hard-coded strings. Feel the subscription path without inventing a second chat stack.',
			where: '/chat · live room · author names',
			label: 'Open chat'
		},
		{
			href: '/blob',
			title: 'Blob game — when the answer must be right now',
			blurb:
				'Moves return the full board state in the same request. Compare this to todos: sometimes you want “strong” command results, sometimes eventual is enough.',
			where: '/blob · board + history · strong return',
			label: 'Play blob'
		},
		{
			href: '/admin',
			title: 'Admin — same app, wider lens',
			blurb:
				'See every owner’s notes and force-archive when needed. Teaches that “what the API allows” follows the signed-in role, not whatever the UI bundle happens to contain.',
			where: '/admin · all owners · admin only',
			label: 'Open admin'
		},
		{
			href: '/session',
			title: 'Session — who am I to the API?',
			blurb:
				'Peek at the signed-in user, groups, and tokens the browser actually holds. Useful when GraphQL “suddenly” returns empty or 401 — start here.',
			where: '/session · identity · tokens',
			label: 'Inspect session'
		}
	];

	/** Framework-wide principles (Distributed as a whole). */
	const frameworkPrinciples = [
		{
			title: 'Simplest DX is the goal',
			body: 'Event sourcing and CQRS are easy to overbuild into a maze of frameworks-on-frameworks. Distributed’s job is the opposite: let you write clear domain intent and ordinary UI, while the library carries the heavy patterns. If a path is all ceremony and no clarity, it is the wrong path.'
		},
		{
			title: 'Start with the domain, not the database',
			body: 'Model a todo or a game as a plain Rust type with methods and unit tests. No HTTP, no SQL, no “handler context” required to prove the rules. When the behavior is solid, plug infrastructure around it — not the other way around.'
		},
		{
			title: 'Know which side of the fence you are on',
			body: 'Writes produce a durable history of what happened. Reads use tables shaped for queries. Messages you publish to other systems go through an outbox on purpose. Handlers stay thin; something else materializes query data; the UI observes. Confusion usually means those lines got blurred.'
		},
		{
			title: 'You keep the interesting code',
			body: 'Macros own the boring event plumbing — recording facts, replaying history, wiring table metadata — so your methods and tests stay readable. You still own the rules (“only the owner can complete this”). The scaffolding disappears; the decisions do not.'
		},
		{
			title: 'Register once, ship everywhere',
			body: 'A command or read model should not be re-described in three hand-maintained places. You register it in the typed Service inventory; generation fans out the GraphQL surface, operation artifacts, command helpers, and checks. When something drifts, CI fails on purpose — better than a quiet production mismatch.'
		},
		{
			title: 'Familiar patterns, short handles',
			body: 'Load an aggregate, apply a command, save events and “please publish this,” project into a query table, send a message on the bus — design patterns you already know, as small swappable APIs. Swap memory for Postgres without rewriting the domain.'
		},
		{
			title: 'Grow without rewriting what you already proved',
			body: 'Start as one process on a laptop. Later split services or change brokers. Transports and features change; the domain types and facts you already tested should not have to be rewritten to match.'
		}
	];

	/** Three teaching cards — prose first, not API dumps. */
	const dxStack = [
		{
			title: 'You write plain domain code',
			body: 'When you model a todo or a game, you write ordinary methods and tests. Macros attach history and replay so you do not hand-roll “record this fact, rebuild state later.” You still see and own the behavior — the scaffolding gets out of the way.'
		},
		{
			title: 'One registration, many surfaces',
			body: 'You describe commands and readable tables once in the typed Rust Service inventory. Generation turns the selected application surface into GraphQL operations, typed UI helpers, optimistic behavior, and checks that fail on drift. You stop maintaining parallel catalogs that slowly disagree.'
		},
		{
			title: 'Patterns as short, swappable verbs',
			body: '“Load it, apply the command, save events and a message to publish” is one small path. Projecting events into query tables and talking on a bus use the same idea: real design patterns, thin handles, backends you can swap when you grow.'
		}
	];

	/** e2e-ui template success rules (apply the framework here). */
	const principles = [
		{
			title: 'Commands change the world; tables are for reading',
			body: 'Do not invent a second write path that updates query tables from the UI “because it’s easier.” Send a command, let the server record history, and let projection (or a deliberate same-request write for games like blob) fill the tables the UI queries.'
		},
		{
			title: 'Trust the signed-in person, not the request body',
			body: 'Who owns a note or who authored a chat message comes from the session. Clients never get to pass “I am alice” as a free-form field. That is how multi-tenant mistakes and spoofing happen.'
		},
		{
			title: 'One client story for data',
			body: 'This app creates one client in the root layout, then reads every route through its replica cache. SSR hydration, HTTP reads, live frames, command results, and optimistic effects all update that same state.'
		},
		{
			title: 'Let the server say how the UI should catch up',
			body: 'Some commands return a full projected row (blob). Others return a fact and wait for a projection or live frame (todos, chat). The typed Service declares that result and optimistic contract so screens do not invent reconciliation logic.'
		},
		{
			title: 'Roles are real, not decorative',
			body: 'An admin can see more and call a few extra mutations. A normal user cannot, even if a type name appears in the generated bundle. Always test with the role you care about.'
		},
		{
			title: 'Regenerate after you change the contract',
			body: 'If you add a command or edit a query document, regenerate and commit the artifacts. Hand-editing generated files is how the next person (or CI) discovers drift the hard way.'
		}
	];

	const crates = [
		{
			name: 'todo-domain / chat-domain / blob-domain',
			role: 'The pure rules — what a todo or game can do, with unit tests and no network'
		},
		{
			name: 'e2e-readmodels',
			role: 'How those facts look when you query them (tables, joins, display fields)'
		},
		{
			name: 'e2e-service',
			role: 'Thin handlers, projectors, identity import, GraphQL surface'
		},
		{
			name: 'e2e-runner → e2e-ui',
			role: 'The process you run — API on :8791'
		},
		{
			name: 'e2e-suite',
			role: 'Automated proof that the paths still work offline and with real OIDC'
		},
		{
			name: 'ui/',
			role: 'SvelteKit app: sign-in, SSR lists, live chat, demos you can click'
		}
	];

	const clientSteps: Array<{
		n: string;
		title: string;
		why: string;
		path: string;
		label: string;
		blocks: CodeBlock[];
	}> = [
		{
			n: 'C1',
			title: 'Declare reads beside the route',
			why: 'A page owns a small GraphQL document and marks when it should load or stay live. The compiler discovers those directives and emits the typed operation plus a static route registry — there is no second list of loaders to maintain.',
			path: 'routes/**/+page.graphql · generated/user/routes.ts',
			label: '@load + @live',
			blocks: [
				{
					file: 'routes/chat/+page.graphql',
					label: 'Co-located read',
					code: `query ChatMessages @load @live {
  chat_messages(where: { room_id: { _eq: "lobby" } }) {
    message_id
    body
    author_id
    author { display_name }
  }
}`
				},
				{
					file: '$distributed',
					label: 'Compiler output',
					code: `export const ChatMessages =
  defineDistributedSvelteKitOperation(Operation_ChatMessages);
export { DISTRIBUTED_ROUTE_OPERATIONS } from './routes.js';
export function provideDistributed(options) { … }
export function useCommands() { … }`
				}
			]
		},
		{
			n: 'C2',
			title: 'The root layout owns SSR',
			why: 'The server adapter matches the current route against the generated registry, runs each declared read once, and serializes reachable replica state. Routes with no declared read do no GraphQL work.',
			path: 'routes/+layout.server.ts',
			label: 'Static route loader',
			blocks: [
				{
					file: 'routes/+layout.server.ts',
					label: 'createDistributedSvelteKitServer',
					code: `const distributed = createDistributedSvelteKitServer({
  routes: DISTRIBUTED_ROUTE_OPERATIONS,
  getSession: (event) => event.locals.auth(),
  getRole: (session) =>
    engineRoleFromGroups(session?.user?.groups),
  getUrl: graphqlHttpUrl
});

export const load = distributed.load;
// Returns replica hydration and a separate authority proof.`
				}
			]
		},
		{
			n: 'C3',
			title: 'Hydrate one browser replica',
			why: 'The root layout creates one client and one session source for reads, live transport, commands, and authorization invalidation. Trusted SSR state becomes the first browser snapshot, so subscribing does not repeat the HTTP read.',
			path: 'routes/+layout.svelte · $distributed',
			label: 'One client',
			blocks: [
				{
					file: 'routes/+layout.svelte',
					label: 'provideDistributed',
					code: `const pageData = createPageDataSessionSource(data);
const client = provideDistributed({
  session: pageData.session,
  browser,
  hydration: data.distributed,
  authority: data.distributedAuthority
});`
				},
				{
					file: 'WS auth',
					label: 'connection_init',
					code: `// The same session source supplies HTTP, WS, and commands.
// A browser token is sent in the first WS message:
{ type: 'connection_init',
  payload: { authorization: \`Bearer \${accessToken}\` } }`
				}
			]
		},
		{
			n: 'C4',
			title: 'Read and write through generated artifacts',
			why: 'A route consumes its generated operation with one use call. Generated commands carry optimistic and causal projection metadata into the same replica, so every mounted view observes the change without page-specific cache surgery.',
			path: 'routes/todos/+page.svelte · $distributed',
			label: 'Operation + commands',
			blocks: [
				{
					file: 'todos/+page.svelte',
					label: 'Replica read',
					code: `import { Todos, useCommands } from '$distributed';

const todos = Todos.use();
const commands = useCommands();

// Template reads the tree-local shared replica: {$todos.data.todos}`
				},
				{
					file: 'todos/+page.svelte',
					label: 'Causal command',
					code: `const receipt = await commands.todo.complete({
  todo_id
});

// Optimism is already visible in every matching Todos view.
// Await this only when later work requires the projection:
await receipt.projected;`
				}
			]
		}
	];

	const serverSteps: Array<{
		n: string;
		title: string;
		why: string;
		path: string;
		label: string;
		blocks: CodeBlock[];
	}> = [
		{
			n: '01',
			title: 'Sign in — your app hosts the form, the IdP issues tokens',
			why: 'People type a password on Fieldnote’s own login page. Zitadel still issues the OIDC tokens; Auth.js keeps them in an encrypted cookie. SSR and GraphQL both use that access token as Bearer.',
			path: 'ui/src/auth.ts · routes/login · Session API v2',
			label: 'Auth session',
			blocks: [
				{
					file: 'ui/src/auth.ts',
					label: 'Session callbacks',
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
			title: 'The first HTML already has your data',
			why: 'The page loader calls GraphQL with your token. The engine checks who you are and only returns rows you may see (for example, notes you own). No empty shell that fetches after paint for the happy path.',
			path: 'service.rs · +page.server.ts',
			label: 'SSR + RBAC',
			blocks: [
				{
					file: 'crates/service/src/service.rs',
					label: 'Row-level RLS',
					code: `// OidcBearer → x-user-id (sub) + engine roles
.model::<TodoView>(
  ModelPermissions::new()
    .role("user", select().all_columns().filter(
      col("owner_id").eq(claim("x-user-id")),
    ))
    .role("admin", select().all_columns()),
)`
				},
				{
					file: 'todos/+page.graphql',
					label: 'Co-located query',
					code: `query Todos @load {
  todos(order_by: [{ status: asc }, { todo_id: asc }]) {
    todo_id
    owner_id
    title
    status
  }
}`
				}
			]
		},
		{
			n: '03',
			title: 'Mutations are commands, not table edits',
			why: 'When the UI says “create todo,” the server runs the domain command and records history. It does not treat GraphQL as a free-form UPDATE of the todos table. Owner always comes from the session.',
			path: 'typed Service inventory · handlers/commands/*',
			label: 'Commands',
			blocks: [
				{
					file: '$distributed',
					label: 'Generated command tree',
					code: `const commands = useCommands();
await commands.todo.create({ title: "Ship it" });
// Also: todo.complete | reopen | archive | rename
// Admin-only: todo.force_archive (only in fieldnote-admin)
// Blob: blob.start | move | start_level
// Chat: chat.post
// owner_id / author_id are NOT client inputs.`
				},
				{
					file: 'handlers/commands/create.rs',
					label: 'Owner from session',
					code: `let owner = require_user(ctx.session())?;
let input = ctx.input::<TodoCreateInput>()?;
todo.create(&input.todo_id, &owner, &input.title)?;
// commit event store + outbox → todo.created
// return fact JSON (status fields) — not a dual-written RM row`
				}
			]
		},
		{
			n: '04',
			title: 'Usually: history first, query tables a moment later',
			why: 'For todos and chat, the command saves the event. A projector turns that event into a row you can query. The UI can show the change optimistically and catch up when the projector lands (or via a live subscription).',
			path: 'handlers/events/project_todo.rs',
			label: 'Projector',
			blocks: [
				{
					file: 'handlers/events/project_todo.rs',
					label: 'Only RM write for todos',
					code: `pub const EVENTS: &[&str] = &[
  "todo.created", "todo.renamed", "todo.completed",
  "todo.reopened", "todo.archived", // + force_archived
];
// decode fact → TodoView → ReadModelWritePlanBuilder upsert
// ChangeHub notifies open subscriptions`
				}
			]
		},
		{
			n: '05',
			title: 'Sometimes: return the full picture in the same request',
			why: 'Games feel broken if the board lags. After blob commits its event, the handler also writes the query row immediately and returns that full row. The UI can trust the response. That is a deliberate “strong” choice — not the default for every feature.',
			path: 'handlers/commands/blob_cmd.rs',
			label: 'CAP · strong',
			blocks: [
				{
					file: 'blob_cmd.rs',
					label: 'Commit + RM',
					code: `ctx.repo().outbox(outbox).commit(game).await?;
// Same request: write the query row so the mutation can return it
let row = map_blob_fact(&fact);
plan.upsert(&row)?;
plan.commit(ctx.read_model_store()).await?;
// GraphQL payload = full board (map, score, status, …)`
				}
			]
		},
		{
			n: '06',
			title: 'People have names — join the identity directory',
			why: 'Chat should show “Alice,” not a raw id. Users are imported from Zitadel into an auth directory table; messages and games join to that. If a user is missing from the directory, fix ingest/scrape — do not invent a second copy of display names on every aggregate.',
			path: 'handlers/ingestors/zitadel · AuthUserView',
			label: 'Joins',
			blocks: [
				{
					file: 'chat_message_view.rs',
					label: 'belongs_to',
					code: `#[readmodel(belongs_to = "AuthUserView", foreign_key = "author_id")]
pub author: Option<AuthUserView>,
// Query selects author { display_name email status }`
				},
				{
					file: 'chat/+page.graphql',
					label: 'Selection',
					code: `query ChatMessages @load @live {
  chat_messages(where: { room_id: { _eq: "lobby" } }) {
    message_id
    body
    author_id
    author { user_id display_name email status }
  }
}`
				}
			]
		}
	];
</script>

<div class="wf-home">
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
				A <em>framework template</em> you run as full e2e tests — a map you can learn from.
			</h1>
			<p class="wf-lede">
				<strong>Simplest DX is the goal.</strong> You should not re-implement event history,
				repositories, and GraphQL wiring for every feature. Distributed carries that weight so you
				can focus on domain rules and a clear UI. This folder is a living suite you can click
				through and copy — sign-in, personal lists, live chat, a game with strong command returns —
				proved by <code>make test</code> offline and <code>make test-live</code> with real OIDC.
			</p>
			<div class="wf-actions">
				{#if signedIn}
					<a class="wf-btn wf-btn-primary" href="/todos">Open todos</a>
					<a class="wf-btn wf-btn-ghost" href="/blob">Blob game</a>
					<a class="wf-btn wf-btn-ghost" href="#framework">Framework principles</a>
				{:else}
					<a class="wf-btn wf-btn-primary" href="/signin?callbackUrl=/todos">Sign in with OIDC</a>
					<a class="wf-btn wf-btn-ghost" href="#framework">Framework principles</a>
				{/if}
			</div>
			<div class="wf-meta">
				<span>tests/e2e-ui</span>
				<span>API :8791</span>
				<span>UI :5180</span>
				<span>Zitadel :18080</span>
			</div>
			<nav class="wf-toc" aria-label="On this page">
				<a href="#framework">Principles</a>
				<a href="#run">Run</a>
				<a href="#demos">Demos</a>
				<a href="#architecture">Architecture</a>
				<a href="#client-dx">Client DX</a>
				<a href="#cap">CAP</a>
				<a href="#server-flow">Server</a>
				<a href="#codegen">Codegen</a>
			</nav>
		</div>
	</section>

	<section class="wf-band wf-band-dark" id="story">
		<div class="wf-band-inner">
			<div class="wf-section-head">
				<span class="wf-label">Why this exists</span>
				<h2>Template first. Product later.</h2>
				<p>
					This is not a product marketing site. It is the face of a fixture that ships with
					Distributed — so when the library’s recommended patterns change, you can see and test them
					here. Copy the folder when you start a service; keep the boundaries that keep you sane.
				</p>
			</div>
			<div class="wf-cards">
				<div class="wf-card">
					<h3>Something you can actually run</h3>
					<p>
						<code>make up</code> starts the database and identity provider.
						<code>make run</code> starts API and UI. Log in as alice, bob, or admin and click
						around.
					</p>
				</div>
				<div class="wf-card">
					<h3>Kept honest by tests</h3>
					<p>
						The same folder has behavioral tests and gated OIDC checks. Offline for day-to-day;
						live stack when you need the real token path. The demos are not slides — they break if
						the library lies.
					</p>
				</div>
				<div class="wf-card">
					<h3>A map you can extend</h3>
					<p>
						Domain crates, read models, thin handlers, suite, Svelte UI. Point env at your database
						and IdP when you are ready; the shape of “command → history → query” stays the same.
					</p>
				</div>
			</div>
		</div>
	</section>

	<section class="wf-band wf-band-light" id="framework">
		<div class="wf-band-inner">
			<div class="wf-section-head wf-section-head-wide">
				<span class="wf-label">Distributed · framework principles</span>
				<h2>What we are optimizing for</h2>
				<p>
					Distributed is a CQRS and event-sourcing framework for Rust people who still want to ship.
					The north star is not “more infrastructure.” It is the
					<strong>simplest developer experience</strong> that keeps write history, queries, and
					published messages honest — so you can grow later without rewriting the domain you already
					proved.
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

			<div class="wf-subhead" id="dx-stack">
				<span class="wf-label">How DX stays simple</span>
				<h3>Three ways the stack carries weight for you</h3>
				<p>
					You should not re-implement “save history,” “sync parallel inventories,” or “build a mini
					framework” for every feature. Three layers share that work. You write intent; the stack
					expands it.
				</p>
			</div>
			<div class="wf-layers">
				{#each dxStack as col, i}
					<div class="wf-layer">
						<span class="wf-layer-n" aria-hidden="true">{String(i + 1).padStart(2, '0')}</span>
						<h4>{col.title}</h4>
						<p class="wf-layer-body">{col.body}</p>
					</div>
				{/each}
			</div>

			<div class="wf-subhead" id="macros">
				<span class="wf-label">When you are ready for the code</span>
				<h3>Domain first — macros fill the seams</h3>
				<p>
					Below is what “plain domain + less noise” looks like in this fixture. Read the method
					bodies as the product rules; the attributes are how history gets recorded and replayed
					without you writing that plumbing twice.
				</p>
			</div>
			<div class="wf-code-stack">
				<div class="wf-code">
					<div class="wf-code-bar">
						<span>todo-domain · models/todo.rs</span>
						<em>one command, one function</em>
					</div>
					<pre><code>{`// One function. #[event] records history and applies state.
// when = skips the command when it must not fire (wrong owner / not open).
#[event("todo.completed", when = self.owner_id == owner_id && self.is_open())]
pub fn complete(&mut self, owner_id: String) {
  self.status = TodoStatus::Completed;
}
// Unit tests call complete() directly — red/green before any handler exists.`}</code></pre>
				</div>
				<div class="wf-code">
					<div class="wf-code-bar">
						<span>e2e-readmodels · TodoView</span>
						<em>#[derive(ReadModel)]</em>
					</div>
					<pre><code>{`#[derive(Clone, Debug, Default, Serialize, Deserialize, ReadModel)]
#[table("todos")]
pub struct TodoView {
  #[id("todo_id")]
  pub todo_id: String,
  pub owner_id: String,
  pub title: String,
  pub status: String,
}
// GraphQL can list and filter this table — you did not hand-write resolvers.`}</code></pre>
				</div>
				<div class="wf-code">
					<div class="wf-code-bar">
						<span>handlers · command input</span>
						<em>GraphqlInput</em>
					</div>
					<pre><code>{`#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct TodoCompleteInput {
  pub todo_id: String,
}
// Same type for GraphQL input and the handler. Register once; codegen follows.`}</code></pre>
				</div>
			</div>

			<div class="wf-subhead" id="pattern-apis">
				<span class="wf-label">Patterns you already know</span>
				<h3>Short verbs you can swap under the hood</h3>
				<p>
					You do not need a new vocabulary for every service. Load something from history, apply a
					command, save events and “please publish this,” turn events into query rows, expose
					handlers. Same ideas as textbooks — thin APIs, backends you can change later.
				</p>
			</div>
			<div class="wf-code-stack">
				<div class="wf-code">
					<div class="wf-code-bar">
						<span>Load → decide → save history</span>
						<em>repository</em>
					</div>
					<pre><code>{`// Load from event history (not from a mutable "current row" only)
let mut todo = ctx.repo().get(todo_id).await?
  .ok_or_else(|| HandlerError::NotFound(todo_id.into()))?;

todo.complete(&owner)?; // domain rules only

// One commit: new events + "please publish this fact"
let fact = TodoFact::from_todo(&todo);
let outbox = OutboxMessage::encode(
  format!("{}:todo.completed:{}", todo.todo_id, todo.entity.version()),
  "todo.completed",
  &fact,
)?;
ctx.repo().outbox(outbox).commit(&mut todo).await?;
// Memory, SQLite, or Postgres — same handler shape.`}</code></pre>
				</div>
				<div class="wf-code">
					<div class="wf-code-bar">
						<span>Turn a fact into something you can query</span>
						<em>projection</em>
					</div>
					<pre><code>{`// After (or with) the event: upsert a row shaped for reads
let row = map_todo_fact(&fact);
let mut plan = ReadModelWritePlanBuilder::new();
plan.upsert(&row)?;
plan.commit(ctx.read_model_store()).await?;
// Same idea for "eventually" (projector) and "right now" (blob).`}</code></pre>
				</div>
				<div class="wf-code">
					<div class="wf-code-bar">
						<span>One handler, many doors</span>
						<em>service</em>
					</div>
					<pre><code>{`// This fixture exposes commands over GraphQL only (no raw POST /todo.*)
Service::new()
  .named("e2e-ui")
  .without_http_command_routes()
  // … register handlers + readable models …
// Later you can also hang the same work off a bus or gRPC
// without rewriting Todo::complete.`}</code></pre>
				</div>
			</div>

			<div class="wf-subhead">
				<span class="wf-label">A calm order of work</span>
				<h3>Prove the domain before the plumbing</h3>
			</div>
			<ol class="wf-flow-map">
				<li>Write tests for what the model should allow and refuse</li>
				<li>Implement the plain type until those tests pass</li>
				<li>Add a thin handler: who is calling → load → domain → save history</li>
				<li>Project into query tables (or same-request for strong UX)</li>
				<li>Expose reads and commands with the right permissions</li>
				<li>Regenerate the client helpers and wire a small UI path</li>
				<li>Swap storage or messaging when you outgrow the laptop setup</li>
			</ol>
		</div>
	</section>

	<section class="wf-band wf-band-light" id="run">
		<div class="wf-band-inner">
			<div class="wf-section-head">
				<span class="wf-label">Run</span>
				<h2>Three commands. Full stack.</h2>
				<p>
					From <code>tests/e2e-ui</code>. Demo password: <code>Password1!</code> (alice / bob /
					admin). After <code>make up</code>, always restart <code>make run</code> so the process
					picks up <code>e2e-ui.env</code>.
				</p>
			</div>
			<div class="wf-steps">
				<div class="wf-step">
					<h3>Bootstrap IdP + DB</h3>
					<p><code>make up</code> writes <code>e2e-ui.env</code>.</p>
				</div>
				<div class="wf-step">
					<h3>API + UI</h3>
					<p>
						<code>set -a && source e2e-ui.env && set +a && make run</code> — GraphQL :8791,
						SvelteKit :5180.
					</p>
				</div>
				<div class="wf-step">
					<h3>Prove it</h3>
					<p>
						<code>make test</code> · <code>make test-live</code> ·
						<code>make check-client</code>
					</p>
				</div>
			</div>
			<div class="wf-subhead">
				<span class="wf-label">Two ways the API knows who you are</span>
				<h3>Pick the profile that matches how you started the process</h3>
			</div>
			<div class="wf-cards wf-cards-tight">
				<div class="wf-card">
					<h3>Local tests (headers)</h3>
					<p>
						When OIDC env is unset, the suite can send simple identity headers. Fast offline
						behavioral tests — that is what <code>make test</code> expects.
					</p>
				</div>
				<div class="wf-card">
					<h3>Real login (Bearer tokens)</h3>
					<p>
						After <code>make up</code> and sourcing env, the API wants real tokens. Browser sessions
						and <code>make test-live</code> use this path. Ambient “I am alice” headers are ignored.
					</p>
				</div>
				<div class="wf-card">
					<h3>Do not mix them</h3>
					<p>
						Hitting an OIDC-only process with header-style tests looks like a wall of 401s. That is
						usually the wrong profile — not a broken product.
					</p>
				</div>
			</div>
		</div>
	</section>

	<section class="wf-band wf-band-dark" id="demos">
		<div class="wf-band-inner">
			<div class="wf-section-head">
				<span class="wf-label">Live demos</span>
				<h2>Practice arenas, not slides</h2>
				<p>
					After <code>make up && make run</code>, sign in and click. Each route is a small story about
					one way the stack behaves — personal data, live rooms, strong returns, admin power,
					identity.
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
		</div>
	</section>

	<section class="wf-band wf-band-light" id="architecture">
		<div class="wf-band-inner">
			<div class="wf-section-head">
				<span class="wf-label">Architecture</span>
				<h2>One direction — with two ways to “see the result”</h2>
				<p>
					The UI never “updates the todos table” as if GraphQL were a database. It asks for work
					(commands) and reads query-shaped data (lists, joins). Most features let query data catch
					up a moment later; the game can return the full board in the same request on purpose.
				</p>
			</div>
			<div class="wf-code wf-code-lead">
				<div class="wf-code-bar">
					<span>Mental model</span>
					<em>system</em>
				</div>
				<pre><code>{`You (browser)
  Sign in → cookie with access token
  Load pages with GraphQL (Bearer)
  Live rooms over WebSocket (token in first message)

Service
  Knows who you are and which role you have
  Commands: change domain history (+ optional “publish this”)
       ├─ todos / chat: project into query tables shortly after  (eventual)
       └─ blob:         also write query row before responding   (strong)
  Queries / live updates: filtered by who you are
  People directory imported from Zitadel for display names`}</code></pre>
			</div>
			<div class="wf-subhead">
				<span class="wf-label">Crate map</span>
				<h3>Where to look when you open the folder</h3>
			</div>
			<dl class="wf-crate-map">
				{#each crates as c}
					<div class="wf-crate">
						<dt>{c.name}</dt>
						<dd>{c.role}</dd>
					</div>
				{/each}
			</dl>
		</div>
	</section>

	<section class="wf-band wf-band-dark" id="principles">
		<div class="wf-band-inner">
			<div class="wf-section-head wf-section-head-wide">
				<span class="wf-label">Template success rules</span>
				<h2>Habits that keep this demo (and your copy of it) healthy</h2>
				<p>
					The principles above are Distributed as a whole. These are the extra habits
					<strong>this fixture</strong> teaches for GraphQL, sign-in, and the Svelte client. Follow
					them and the demos feel calm; fight them and you spend the day on thrash and spoofing.
				</p>
			</div>
			<ol class="wf-principles">
				{#each principles as p, i}
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

	<section class="wf-band wf-band-light" id="client-dx">
		<div class="wf-band-inner">
			<div class="wf-section-head">
				<span class="wf-label">Client GraphQL DX</span>
				<h2>How the browser is supposed to feel</h2>
				<p>
					You should not wire a new HTTP client per page. The root layout creates one replica from
					one session source; the server hydrates it, <code>@live</code> operations continue it, and
					typed commands update it optimistically before causal projection catches up. Query
					documents live next to routes, while command and optimistic contracts come from the same
					typed Rust Service inventory the API runs.
				</p>
			</div>
			<ol class="wf-flow-map">
				<li>Declare route reads with <code>@load</code> and <code>@live</code></li>
				<li>Let the generated static registry drive SSR</li>
				<li>Hydrate one browser replica with separate authority proof</li>
				<li>Read with generated <code>Todos.use()</code></li>
				<li>Write through generated nested commands</li>
				<li>Let one auth source fence HTTP, WebSocket, commands, and cached scope</li>
			</ol>

			<div class="wf-chapter">
				{#each clientSteps as step}
					<article class="wf-story-step" id="client-{step.n}" data-sample={step.label}>
						<div class="wf-story-copy">
							<span class="wf-label">Client {step.n}</span>
							<h3 class="wf-step-title">{step.title}</h3>
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
					</article>
				{/each}
			</div>
		</div>
	</section>

	<section class="wf-band wf-band-dark" id="cap">
		<div class="wf-band-inner">
			<div class="wf-section-head">
				<span class="wf-label">When should the answer be “now”?</span>
				<h2>Two honest styles — pick per feature</h2>
				<p>
					Not every screen needs the same consistency story. This fixture shows both on purpose so
					you can feel the tradeoff instead of arguing in the abstract.
				</p>
			</div>
			<div class="wf-cards">
				<div class="wf-card wf-card-accent">
					<span class="wf-card-kicker">Strong</span>
					<h3>Blob — the board is the truth of the response</h3>
					<p>
						After a move, the mutation comes back with the full board. Waiting on a background
						projector would feel like lag. The UI keeps one cache for board and history so they
						cannot disagree.
					</p>
				</div>
				<div class="wf-card">
					<span class="wf-card-kicker">Eventual</span>
					<h3>Todos — feel instant, settle a moment later</h3>
					<p>
						Completing a note can paint as done immediately. The server still records history first;
						the generated optimistic effect remains in the replica until a causal result or later
						projection proves the query table caught up.
					</p>
				</div>
				<div class="wf-card">
					<span class="wf-card-kicker">Eventual + live</span>
					<h3>Chat — other people are the clock</h3>
					<p>
						Your post can show immediately; everyone else’s posts arrive over a live subscription.
						That open connection is how the room converges — not a frantic poll loop.
					</p>
				</div>
			</div>
		</div>
	</section>

	<section class="wf-band wf-band-light" id="server-flow">
		<div class="wf-band-inner">
			<div class="wf-section-head">
				<span class="wf-label">Server path</span>
				<h2>From “I signed in” to “I can see my data”</h2>
				<p>
					Walk this when you are implementing a new feature. The client steps above are how the UI
					consumes the same loop.
				</p>
			</div>
			<ol class="wf-flow-map">
				<li>Person signs in; session holds a token</li>
				<li>Page load queries with that token (only their rows)</li>
				<li>UI sends a command, not a free-form table update</li>
				<li>Handler loads history, applies domain rules, saves events</li>
				<li>Query tables update eventually — or immediately for strong UX</li>
				<li>Display names join from the identity directory</li>
				<li>Queries and live rooms show the new world</li>
			</ol>

			<div class="wf-chapter">
				{#each serverSteps as step}
					<article class="wf-story-step" id="step-{step.n}" data-sample={step.label}>
						<div class="wf-story-copy">
							<span class="wf-label">Server {step.n}</span>
							<h3 class="wf-step-title">{step.title}</h3>
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
					</article>
				{/each}
			</div>
		</div>
	</section>

	<section class="wf-band wf-band-dark" id="codegen">
		<div class="wf-band-inner">
			<div class="wf-section-head">
				<span class="wf-label">Keeping the UI honest</span>
				<h2>One inventory in, one complete client out</h2>
				<p>
					The typed Rust Service inventory defines readable models, commands, permissions, results,
					and optimistic effects. <code>dctl client-manifest</code> extracts a selected application
					surface without a database; <code>dctl client</code> combines it with co-located route
					reads. Drift fails a check instead of becoming a production surprise.
				</p>
			</div>
			<div class="wf-cards wf-cards-tight">
				<div class="wf-card">
					<h3>Application surfaces are capabilities</h3>
					<p>
						The pool-free <code>distributed_client_surface</code> exports
						<code>fieldnote</code> for admin and user roles. A separate
						<code>fieldnote-admin</code> export is consumed only by the nested admin layout.
					</p>
				</div>
				<div class="wf-card">
					<h3>Routes declare reads, Rust declares writes</h3>
					<p>
						Use <code>+page.graphql</code> with <code>@load</code> or
						<code>@live</code>. Generation emits inspectable artifacts, tree-local operation
						wrappers, the static route registry, and causal command bindings.
					</p>
				</div>
				<div class="wf-card">
					<h3>After you change the contract</h3>
					<p>
						Run <code>make gen-client</code>, inspect and commit the generated artifacts, then let
						<code>make check-client</code> enforce byte and file-set drift. Durable design belongs
						in the Distributed GitKB; this fixture stays an executable example.
					</p>
				</div>
			</div>
			<div class="wf-code wf-code-follow">
				<div class="wf-code-bar">
					<span>What a day-to-day write looks like</span>
					<em>UI</em>
				</div>
				<pre><code>{`import { Todos, useCommands } from '$distributed';

const todos = Todos.use();
const commands = useCommands();

await commands.todo.create({ title }); // todo_id defaults to uuid_v7()
await commands.todo.reopen({ todo_id });
await commands.chat.post({ message_id, body, room_id, created_at });
await commands.blob.move({ game_id, direction: 'up' });

// Inside the nested admin tree:
import { useCommands as useAdminCommands } from '$distributed/admin';
await useAdminCommands().todo.force_archive({ todo_id });`}</code></pre>
			</div>
		</div>
	</section>

	<section class="wf-band wf-band-light" id="auth-joins">
		<div class="wf-band-inner">
			<div class="wf-section-head">
				<span class="wf-label">People have names</span>
				<h2>Import identity once, join everywhere</h2>
				<p>
					Do not stamp a free-form display name onto every chat message forever. Import people from
					the IdP into a directory table, then join. Fix missing people at the source (ingest /
					scrape), not by inventing a second source of truth on each aggregate.
				</p>
			</div>
			<div class="wf-cards">
				<div class="wf-card">
					<h3>Bring users in</h3>
					<p>
						Zitadel can push or you can scrape; either way facts land as directory rows. See
						<code>docs/zitadel-ingestor.md</code> when you need the exact endpoints.
					</p>
				</div>
				<div class="wf-card">
					<h3>Join for labels</h3>
					<p>
						Chat shows an author; blob can show an owner. Those are relationships to the directory
						— so a rename or status change does not require rewriting history.
					</p>
				</div>
				<div class="wf-card">
					<h3>Roles in practice</h3>
					<p>
						alice and bob are normal users (their own todos). admin sees everyone and can
						force-archive. Groups on the session map into those engine roles.
					</p>
				</div>
			</div>
		</div>
	</section>

	<section class="wf-band wf-band-dark" id="anti-patterns">
		<div class="wf-band-inner">
			<div class="wf-section-head">
				<span class="wf-label">Traps</span>
				<h2>Things that feel clever and then hurt</h2>
			</div>
			<div class="wf-cards">
				<div class="wf-card">
					<h3>GraphQL as a free-form table editor</h3>
					<p>
						If the UI can UPDATE the todos table directly, you have two write paths and no single
						history. Prefer commands that go through the domain.
					</p>
				</div>
				<div class="wf-card">
					<h3>Two lists for the same data</h3>
					<p>
						A local array “for convenience” plus the shared cache for the same list is how board
						and history disagree. Use one replica-backed operation view for that data.
					</p>
				</div>
				<div class="wf-card">
					<h3>Hard refetch that yanks the optimistic UI</h3>
					<p>
						Blasting a full reload the instant a command returns can flash, reorder, or erase the
						row you just painted. Let generated causal optimism remain until the replica observes
						projection or a live frame.
					</p>
				</div>
				<div class="wf-card">
					<h3>Believing the client about who they are</h3>
					<p>
						Owner and author come from the session. Client-supplied “I am alice” fields are how
						multi-tenant bugs are born.
					</p>
				</div>
				<div class="wf-card">
					<h3>Hand-editing generated files</h3>
					<p>
						Change the real source (Rust registry or co-located query), regenerate, commit. CI
						checks exist so drift does not wait for a human on-call.
					</p>
				</div>
				<div class="wf-card">
					<h3>Tokens in the wrong place on WebSockets</h3>
					<p>
						Browsers cannot set Authorization on the upgrade the way they do on HTTP. Put the
						token in the first connection message — never in a long-lived URL query string.
					</p>
				</div>
			</div>
		</div>
	</section>

	<section class="wf-band wf-band-light" id="extend">
		<div class="wf-band-inner">
			<div class="wf-section-head">
				<span class="wf-label">Extend the template</span>
				<h2>Adding a feature without losing the plot</h2>
			</div>
			<ol class="wf-flow-map">
				<li>Model the rules in a pure domain crate with tests</li>
				<li>Describe how those facts look when queried (and any joins)</li>
				<li>Thin command handler + register its typed result and optimistic contract</li>
				<li>Projector — unless you deliberately write the query row in-request</li>
				<li>Permissions: who may read which rows</li>
				<li>Add a co-located <code>+page.graphql</code> and regenerate client artifacts</li>
				<li>Small UI path: generated <code>View.use()</code> + generated command</li>
				<li>A test that would fail if you regressed the story</li>
			</ol>
		</div>
	</section>

	<section class="wf-band wf-band-dark wf-cta-band">
		<div class="wf-band-inner wf-cta">
			<h2>Go click around</h2>
			<p>
				Sign in (alice / bob / admin · Password1!). Feel todos settle after a moment, blob answer
				immediately, chat fill from others, admin see everyone, session explain who you are.
			</p>
			<div class="wf-actions">
				{#if signedIn}
					<a class="wf-btn wf-btn-primary" href="/todos">Todos</a>
					<a class="wf-btn wf-btn-ghost" href="/blob">Blob</a>
					<a class="wf-btn wf-btn-ghost" href="/chat">Chat</a>
					<a class="wf-btn wf-btn-ghost" href="/session">Session</a>
				{:else}
					<a class="wf-btn wf-btn-primary" href="/signin?callbackUrl=/todos">Sign in</a>
					<a class="wf-btn wf-btn-ghost" href="#principles">Success rules</a>
				{/if}
			</div>
		</div>
	</section>

	<Footer />
</div>
