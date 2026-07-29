<script lang="ts">
	/**
	 * e2e-ui home — living walkthrough of how e2e-ui actually works.
	 *
	 * Keep samples honest: one file / one concept per code block, and match the
	 * checked-in handlers, GraphQL docs, and dual client surfaces.
	 */
	import '$lib/styles/home.css';
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
			title: 'Todos — owner-scoped list',
			blurb:
				'Create, complete, reopen, archive, or purge. One domain-event projection drives the server row, optimistic preview, and causal confirmation. You only see your own todos unless you are admin.',
			where: '/todos · Causal + modeled projection · personal RLS',
			label: 'Open todos'
		},
		{
			href: '/chat',
			title: 'Lobby chat — one doc for SSR and live',
			blurb:
				'One @load @live query seeds HTML and continues over GraphQL WebSocket. Post is a command; everyone else arrives as live frames — not a second chat stack.',
			where: '/chat · @load @live · shared room',
			label: 'Open chat'
		},
		{
			href: '/blob',
			title: 'Blob game — Projected board in the response',
			blurb:
				'Each move is an aggregate command that returns Projected<BlobGames>. Map and score commit with the event; the replica applies the payload before the call resolves.',
			where: '/blob · Projected · no dual-write',
			label: 'Play blob'
		},
		{
			href: '/admin',
			title: 'Admin — separate generated surface',
			blurb:
				'e2e-ui-admin is a second client (nested layout + role gate). Elevated ops are not discoverable from the user $distributed tree.',
			where: '/admin · e2e-ui-admin · force-archive',
			label: 'Open admin'
		},
		{
			href: '/session',
			title: 'Session — who am I to the API?',
			blurb:
				'User, groups → engine role, and the tokens the browser actually holds. Start here when GraphQL is empty or 401.',
			where: '/session · identity · tokens',
			label: 'Inspect session'
		}
	];

	/** Framework-wide principles (Distributed as a whole). */
	const frameworkPrinciples = [
		{
			title: 'Simplest DX is the goal',
			body: 'Event sourcing and CQRS are easy to overbuild. Distributed’s job is the opposite: plain domain intent and ordinary UI, while the library carries history, projection, GraphQL, and the browser replica. Ceremony without clarity is the wrong path.'
		},
		{
			title: 'Start with the domain, not the database',
			body: 'Model a todo or game as a plain Rust type with methods and unit tests. No HTTP, no SQL, no handler context to prove the rules. Infrastructure plugs in after the behavior is solid.'
		},
		{
			title: 'Know which side of the fence you are on',
			body: 'Aggregate events replay write-side history. Domain events are the outward contract other components may react to; a sourced event can automatically publish its post-transition DomainState when that is the useful contract. Read models stay query-shaped.'
		},
		{
			title: 'You keep the interesting code',
			body: 'Macros own recording facts and replaying history so methods stay readable. You still own “only the owner can complete this.” Scaffolding disappears; decisions do not.'
		},
		{
			title: 'Register once, ship everywhere',
			body: 'Commands, domain events, and projections live once in the typed Service inventory. Generation lowers the same projection into server execution, optimistic client effects, causal obligations, GraphQL fields, and dual application surfaces. Drift fails CI on purpose.'
		},
		{
			title: 'Familiar patterns, short handles',
			body: 'Load aggregate → apply command → publish_events/project → commit → Causal or Projected result. Swap memory for Postgres without rewriting the domain.'
		},
		{
			title: 'Grow without rewriting what you proved',
			body: 'Start as one process on a laptop. Later split services or change brokers. Domain types and facts you already tested should not need a rewrite.'
		}
	];

	/** Three teaching cards — prose first, not API dumps. */
	const dxStack = [
		{
			title: 'You write plain domain code',
			body: 'Ordinary methods and tests. Public command methods enforce rules; private #[event] helpers record history. You own the behavior — the scaffolding gets out of the way.'
		},
		{
			title: 'One inventory, many surfaces',
			body: 'Describe commands, domain events, projection programs, and readable tables once. Application surfaces select roles and fields. Generation turns that into GraphQL, typed UI helpers, safe optimistic effects, and drift checks.'
		},
		{
			title: 'Patterns as short, swappable verbs',
			body: 'CausalCommandContext: load / create / publish_events / project / commit. Real design patterns, thin handles, backends you can swap when you grow.'
		}
	];

	/** e2e-ui template success rules (apply the framework here). */
	const principles = [
		{
			title: 'Commands change the world; tables are for reading',
			body: 'Do not dual-write query tables from the UI. Send a command. Todos/chat return Causal and project eventually; blob returns Projected in the same transaction as the event.'
		},
		{
			title: 'Trust the signed-in person, not the request body',
			body: 'Owner and author come from the session (ctx.user_id()). Clients never pass “I am alice” as free-form input.'
		},
		{
			title: 'One replica story for user data',
			body: 'Root layout creates one client. SSR hydration, HTTP reads, @live frames, command results, and optimistic effects all update that same replica. Admin is a second nested client on purpose.'
		},
		{
			title: 'Let the Service declare how the UI catches up',
			body: 'A command names the domain events it may emit and optionally previews values known from input, generated defaults, trusted claims, or constants. The compiler derives safe optimism and causal obligations; unknown fields fall back to revalidation.'
		},
		{
			title: 'Roles and surfaces are real',
			body: 'Engine RLS filters rows. Elevated mutations live only on e2e-ui-admin. A type name in a bundle is not authorization.'
		},
		{
			title: 'Regenerate after you change the contract',
			body: 'Inventory, command contract, or +page.graphql change → make gen-client / check-client and commit artifacts. Hand-editing generated files is how drift wins.'
		}
	];

	const crates = [
		{
			name: 'todo-domain / chat-domain / blob-domain',
			role: 'Rules, DomainState, natural read models, and portable projection programs'
		},
		{
			name: 'e2e-readmodels',
			role: 'Provider-owned auth_users plus deployment-composed cross-domain joins'
		},
		{
			name: 'e2e-service',
			role: 'Fluent commits, projection catalog/placement, dual client surfaces, GraphQL'
		},
		{
			name: 'e2e-runner → e2e-ui',
			role: 'Process on :8791 (Postgres + OidcBearer or SQLite + DevHeaders)'
		},
		{
			name: 'e2e-suite',
			role: 'Behavioral + gated OIDC proof'
		},
		{
			name: 'ui/',
			role: 'SvelteKit: OIDC, SSR @load, live chat, blob, admin surface'
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
			why: 'A page owns a small GraphQL document. @load seeds SSR; @live continues the same operation over WebSocket. The compiler emits the typed op and the static route registry — no second loader list.',
			path: 'routes/**/+page.graphql · generated/user/',
			label: '@load + @live',
			blocks: [
				{
					file: 'routes/chat/+page.graphql',
					label: 'Co-located read',
					code: `// One declaration owns SSR, cache reads, and the live companion.
query ChatMessages @load @live {
  chat_messages(where: { room_id: { _eq: "lobby" } }) {
    message_id
    room_id
    author_id
    body
    created_at
  }
}`
				},
				{
					file: 'routes/blob/[[gameId]]/+page.graphql',
					label: 'Join on load',
					code: `query BlobGames @load {
  blob_games(order_by: [{ game_id: asc }]) {
    game_id
    score
    map_json
    owner { user_id display_name }
  }
}`
				}
			]
		},
		{
			n: 'C2',
			title: 'The root layout owns SSR',
			why: 'createDistributedSvelteKitServer matches the route against DISTRIBUTED_ROUTE_OPERATIONS, runs each declared read once, and returns replica hydration plus a separate authority proof. Routes with no @load do no GraphQL work.',
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
// → data.distributed + data.distributedAuthority`
				}
			]
		},
		{
			n: 'C3',
			title: 'Hydrate one browser replica',
			why: 'provideDistributed wires one client for HTTP, WebSocket connection_init, commands, and auth-scope invalidation. SSR state is the first snapshot so subscribing does not re-fetch the same HTTP read.',
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
});
// Navigation: client.hydrate(…) after the session fence`
				},
				{
					file: 'WS auth',
					label: 'connection_init',
					code: `// Same session source for HTTP, WS, and commands.
// Token goes in the first WS message — not the upgrade URL:
{ type: 'connection_init',
  payload: { authorization: \`Bearer \${accessToken}\` } }`
				}
			]
		},
		{
			n: 'C4',
			title: 'Read and write through generated artifacts',
			why: 'Todos.use() / ChatMessages.use() / BlobGames.use() bind the tree-local replica. useCommands() carries projection-derived optimistic operations and Projected/Causal metadata so every mounted view observes the change without page-specific cache surgery.',
			path: 'routes/todos/+page.svelte · $distributed',
			label: 'Operation + commands',
			blocks: [
				{
					file: 'todos/+page.svelte',
					label: 'Replica read + command',
					code: `import { Todos, useCommands } from '$distributed';

const list = Todos.use();
const commands = useCommands();
const rows = $derived($list.complete ? $list.data.todos : []);

await commands.todo.create({ title });
// todo_id defaults to uuid_v7() from the inventory
await commands.todo.complete({ todo_id });
// Modeled optimism is already visible; the exact causal obligation confirms later`
				},
				{
					file: 'blob/[[gameId]]/+page.svelte',
					label: 'Projected move',
					code: `const list = BlobGames.use();
const commands = useCommands();

const receipt = await commands.blob.move({
  game_id,
  direction: 'up'
});
// consistency: "projected" — board hits the replica
// from the mutation payload before this resolves`
				}
			]
		},
		{
			n: 'C5',
			title: 'Elevated work is a second surface',
			why: 'e2e-ui-admin is generated separately. The nested /admin layout role-gates before any GraphQL, then provides its own client. User pages cannot import force_archive through $distributed.',
			path: 'routes/admin/+layout.server.ts · $distributed/admin',
			label: 'Dual surface',
			blocks: [
				{
					file: 'admin/+layout.server.ts',
					label: 'Role gate + admin registry',
					code: `import { DISTRIBUTED_ROUTE_OPERATIONS } from '$distributed/admin';

const distributed = createDistributedSvelteKitServer({
  routes: DISTRIBUTED_ROUTE_OPERATIONS,
  getRole: (session) => {
    const role = engineRoleFromGroups(session?.user?.groups);
    if (!isAdminEngineRole(role)) error(403, 'Admin role required');
    return role;
  },
  // …
});`
				},
				{
					file: 'admin page',
					label: 'Elevated commands only',
					code: `import { useCommands } from '$distributed/admin';
const commands = useCommands();
await commands.todo.force_archive({ todo_id });
// Not present on the user command tree`
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
			title: 'Sign in — your form, IdP tokens',
			why: 'Custom Login V2 hosts the password form; Zitadel issues OIDC tokens; Auth.js keeps them in an encrypted cookie. SSR and GraphQL use the access token as Bearer. Groups map to engine roles.',
			path: 'ui/src/auth.ts · routes/login · zitadel-session.ts',
			label: 'Auth session',
			blocks: [
				{
					file: 'ui/src/auth.ts',
					label: 'Session callbacks',
					code: `// Auth.js OIDC (Zitadel) — PKCE + state
// Tokens in encrypted JWT session cookie (httpOnly)
callbacks: {
  async jwt({ token, account }) {
    if (account) {
      token.accessToken = account.access_token;
      token.refreshToken = account.refresh_token;
      token.expiresAt = account.expires_at ?? …;
    }
    // silent refresh before expiry when refresh_token present
    return token;
  },
  async session({ session, token }) {
    session.accessToken = token.accessToken; // API Bearer
    session.user.id = token.sub;
    session.user.groups = /* from IdP claims */;
    return session;
  }
}`
				}
			]
		},
		{
			n: '02',
			title: 'SSR GraphQL is deny-by-default RLS',
			why: 'The root loader calls GraphQL with your token. OidcBearer maps JWT → x-user-id + roles. Model permissions filter rows — alice never sees bob’s todos or blob games.',
			path: 'crates/service/src/service.rs · +page.graphql',
			label: 'SSR + RLS',
			blocks: [
				{
					file: 'crates/service/src/service.rs',
					label: 'Row-level grants',
					code: `// OidcBearer → x-user-id (sub) + engine roles
.model::<Todos>(
  ModelPermissions::new()
    .grant(
      "user",
      read().all_columns().rows(
        col("owner_id").eq(claim("x-user-id")),
      ),
    )
    .grant("admin", read().all_columns()),
)
// Same pattern for BlobGames; ChatMessages is room-shared`
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
			title: 'Mutations are commands — Causal or Projected',
			why: 'The typed Service inventory declares input, result shape, roles, emitted domain events, and any safe pre-dispatch preview. The projection program derives optimism and exact causal obligations. Handlers never accept owner_id from the client.',
			path: 'service.rs · handlers/commands/*',
			label: 'Commands',
			blocks: [
				{
					file: 'handlers/commands/create.rs',
					label: 'Causal + session owner',
					code: `pub async fn handle(
  ctx: &CausalCommandContext<'_, Todo>,
  input: TodoCreateInput,
) -> Result<PreparedCommand<Causal<TodoCreatePayload>>, HandlerError> {
  let owner = ctx.user_id()?.to_string(); // never from input
  let mut todo = ctx.create();
  todo.create(&input.todo_id, &owner, &input.title)?;
  commit_todo_events(ctx, todo, |state| {
    TodoCreatePayload {
      todo_id: state.todo_id, owner_id: state.owner_id,
      title: state.title, status: state.status,
    }
  })
}`
				},
				{
					file: 'service.rs · typed_command',
					label: 'Event + safe preview',
					code: `typed_command::<TodoCreateInput, Causal<TodoCreatePayload>>(…)
  .emits(events![TodoCreatedDomainEvent])
  .preview(state_preview! {
    TodoCreatedDomainEvent => TodoState {
      todo_id: generated.todo_id,
      owner_id: trusted("x-user-id", "string"),
      title: input.title,
      status: "open",
      assignee_id: null,
    }
  })`
				}
			]
		},
		{
			n: '04',
			title: 'Eventual path: one projection, mounted as a consumer',
			why: 'Todos and chat publish captured domain events with the fluent commit builder. The catalog-pinned projection executes the same portable operations the client preview specialized; ChangeHub wakes @live subscribers.',
			path: 'todo-domain/projection.rs · service.rs',
			label: 'Modeled projector',
			blocks: [
				{
					file: 'todo-domain/projection.rs',
					label: 'State lifecycle + deletion',
					code: `pub const TODO_READS: ProjectionDescriptor<EventualOnly> = projection! {
  name: "project_todos";
  epoch: "e2e-ui-todos-v2";

  on ["todo.created", "todo.renamed", "todo.completed", …]
    version 1 (state: TodoState) {
      upsert Todos from state as todo;
  }
  on "todo.purged" version 1 (deleted: TodoDomainIdentity) {
    delete Todos { key { todo_id: envelope.aggregate_id } };
  }
};

// Routes::new().consume_projection(catalog_pinned_todo_owner)`
				}
			]
		},
		{
			n: '05',
			title: 'Strong path: Projected in the same transaction',
			why: 'Blob maps must not lag. The handler returns PreparedCommand<Projected<BlobGames>> through the fluent direct-projection commit — aggregate, ledger, and query row commit together. No second writer, no dual-write from the UI.',
			path: 'handlers/commands/blob_move.rs · blob_cmd.rs',
			label: 'Projected',
			blocks: [
				{
					file: 'handlers/commands/blob_move.rs',
					label: 'Projected result',
					code: `pub async fn handle(
  ctx: &CausalCommandContext<'_, BlobGame>,
  input: BlobMoveInput,
) -> Result<PreparedCommand<Projected<BlobGames>>, HandlerError> {
  let owner = ctx.user_id()?.to_string();
  let mut game = load_game(ctx, &input.game_id).await?;
  game.move_dir(&owner, dir).map_err(map_domain)?;
  commit_blob(ctx, game)
}`
				},
				{
					file: 'service.rs · SurfaceDirectProjection',
					label: 'Client topology',
					code: `fn blob_projection() -> SurfaceDirectProjection {
  SurfaceDirectProjection::new("project_blob")
    .modeled(catalog.resolve(BLOB_GAMES))
}
// typed_command::<…, Projected<BlobGames>>(…)`
				}
			]
		},
		{
			n: '06',
			title: 'People have names — join the identity directory',
			why: 'Import users from Zitadel into auth_users; join from chat.author and blob.owner. Fix missing people at ingest/scrape — do not copy display names onto every aggregate.',
			path: 'handlers/ingestors/zitadel · AuthUserView',
			label: 'Joins',
			blocks: [
				{
					file: 'readmodels/src/lib.rs',
					label: 'deployment-composed belongs_to',
					code: `let mut schema = BlobGames::schema().clone();
schema.relationships.push(RelationshipDef {
  field_name: "owner".into(),
  kind: RelationshipKind::BelongsTo,
  target_model: "AuthUserView".into(),
  foreign_key: Some("owner_id".into()),
  …
});
// Projection storage identity remains the canonical BlobGames row.`
				},
				{
					file: 'readmodels/models/auth_user_view.rs',
					label: 'reverse relationships',
					code: `#[readmodel(has_many = "ChatMessages", foreign_key = "author_id")]
pub chat_messages: Vec<ChatMessages>,
#[readmodel(has_many = "BlobGames", foreign_key = "owner_id")]
pub blob_games: Vec<BlobGames>,
// Select author/owner or reverse collections without a crate cycle.`
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
				<strong>Simplest DX is the goal.</strong> Plain domains, deny-by-default GraphQL with
				first-class OIDC, dual generated clients, and a causal browser replica. Click through todos
				(<code>Causal</code> + projector), chat (<code>@load @live</code>), blob
				(<code>Projected</code>), and admin (separate surface) — proved by
				<code>make test</code> offline and Playwright with real OIDC.
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
				<a href="#cap">Causal vs Projected</a>
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
						Domain tests, behavioral suite, gated OIDC, and Playwright browser flows. The demos are
						not slides — they break if the library lies.
					</p>
				</div>
				<div class="wf-card">
					<h3>A map you can extend</h3>
					<p>
						Domain crates, read models, thin handlers, dual client surfaces, Svelte UI. Point env at
						your database and IdP; the shape of command → history → query stays the same.
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
					Public methods enforce rules; private <code>#[event]</code> helpers record history. Unit
					tests call the domain directly — red/green before any handler exists.
				</p>
			</div>
			<div class="wf-code-stack">
				<div class="wf-code">
					<div class="wf-code-bar">
						<span>todo-domain · models/todo.rs</span>
						<em>rules then record</em>
					</div>
					<pre><code>{`pub fn complete(&mut self, owner_id: &str) -> Result<(), TodoError> {
  self.ensure_owner(owner_id)?;
  self.require_mutable()?;
  if matches!(self.status, TodoStatus::Completed) {
    return Err(TodoError::AlreadyCompleted);
  }
  self.record_completed()?;
  Ok(())
}

#[event("todo.completed", version = 1, domain)]
fn record_completed(&mut self) {
  self.status = TodoStatus::Completed;
}
// The aggregate event replays history; `domain` also captures TodoState
// as the outward domain-event body after the transition.`}</code></pre>
				</div>
				<div class="wf-code">
					<div class="wf-code-bar">
						<span>todo-domain · TodoState + Todos</span>
						<em>DomainState + ReadModel</em>
					</div>
					<pre><code>{`#[derive(Clone, Debug, Serialize, Deserialize, DomainState)]
pub struct TodoState {
  // Public post-transition DTO; may omit private snapshot fields.
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, ReadModel)]
#[readmodel(primary_key = ["todo_id"])]
pub struct Todos {
  #[readmodel(id)]
  pub todo_id: String,
  pub owner_id: String,
  pub title: String,
  pub status: String,
}
// GraphQL lists/filters `todos` — no hand-written resolvers.`}</code></pre>
				</div>
				<div class="wf-code">
					<div class="wf-code-bar">
						<span>handlers · command input</span>
						<em>GraphqlInput</em>
					</div>
					<pre><code>{`#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct TodoCreateInput {
  pub todo_id: String,
  pub title: String,
}
// owner_id is NOT an input — handler uses ctx.user_id().
// Same type for GraphQL input and the handler.`}</code></pre>
				</div>
			</div>

			<div class="wf-subhead" id="pattern-apis">
				<span class="wf-label">Patterns you already know</span>
				<h3>Short verbs you can swap under the hood</h3>
				<p>
					Load from history, apply a command, publish captured domain events, return
					<code>Causal</code> or <code>Projected</code>. Same ideas as textbooks — thin APIs, backends
					you can change later.
				</p>
			</div>
			<div class="wf-code-stack">
				<div class="wf-code">
					<div class="wf-code-bar">
						<span>Load → decide → fluent commit</span>
						<em>CausalCommandContext</em>
					</div>
					<pre><code>{`let mut todo = load_todo(ctx, &input.todo_id).await?;
todo.complete(&owner).map_err(map_domain)?;
let state = TodoState::from(&*todo);
ctx.publish_events()
  .commit(todo)?
  .causal(TodoStatusPayload {
    todo_id: state.todo_id, status: state.status
  })
// Framework commits history + publication + ledger together.`}</code></pre>
				</div>
				<div class="wf-code">
					<div class="wf-code-bar">
						<span>Projected — same transaction</span>
						<em>blob</em>
					</div>
					<pre><code>{`ctx.project(BLOB_GAMES)
  .commit(game)?
  .projected(view)
// → PreparedCommand<Projected<BlobGames>>
// Aggregate + command ledger + query row commit atomically.
// No manual ReadModelWritePlan in the handler.`}</code></pre>
				</div>
				<div class="wf-code">
					<div class="wf-code-bar">
						<span>One handler inventory, GraphQL door</span>
						<em>service</em>
					</div>
					<pre><code>{`// This fixture exposes commands over GraphQL only
// (POST /todo.* must 404 — suite proves it)
Service::new()
  .named("e2e-ui")
  .without_http_command_routes()
  .routes(todos)
  .routes(chat)
  .routes(blob);
// Later: bus / gRPC without rewriting Todo::complete`}</code></pre>
				</div>
			</div>

			<div class="wf-subhead">
				<span class="wf-label">A calm order of work</span>
				<h3>Prove the domain before the plumbing</h3>
			</div>
			<ol class="wf-flow-map">
				<li>Write tests for what the model should allow and refuse</li>
				<li>Implement the plain type until those tests pass</li>
				<li>Thin handler: session → load/create → domain → stage (+ Causal or Projected)</li>
				<li>Projector for eventual models; SurfaceDirectProjection for Projected</li>
				<li>Permissions + client application surface(s)</li>
				<li>Co-located <code>+page.graphql</code>, <code>make gen-client</code>, thin UI</li>
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
						<code>make test-browser</code> · <code>make check-client</code>
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
					one way the stack behaves — personal data, live rooms, Projected returns, elevated surface,
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
				<h2>One direction — two result shapes</h2>
				<p>
					The UI never “updates the todos table” as if GraphQL were a database. It asks for work
					(commands) and reads query-shaped data. Most features return a <code>Causal</code> and let
					projectors catch up; blob returns <code>Projected</code> so the board is in the mutation
					payload.
				</p>
			</div>
			<div class="wf-code wf-code-lead">
				<div class="wf-code-bar">
					<span>Mental model</span>
					<em>system</em>
				</div>
				<pre><code>{`You (browser)
  Sign in → cookie with access token
  @load pages → GraphQL (Bearer) → SSR hydrate replica
  @live rooms → WebSocket (token in connection_init)
  commands.* → same replica (projection preview / authoritative delta)

Service
  OidcBearer → x-user-id + roles · deny-by-default RLS
  typed Service inventory → GraphQL mutations + client surfaces
       ├─ todos / chat: Causal + modeled projector (+ @live) (eventual)
       └─ blob:         Projected<BlobGames>                 (atomic)
  e2e-ui vs e2e-ui-admin application surfaces
  auth_users imported from Zitadel for joins`}</code></pre>
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
					<strong>this fixture</strong> teaches for GraphQL, sign-in, dual surfaces, and the Svelte
					client. Follow them and the demos feel calm; fight them and you spend the day on thrash and
					spoofing.
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
					typed commands update it (projection-derived optimism or Projected payload). Query documents live
					next to routes; command contracts come from the same typed Rust inventory the API runs.
					Admin is a second generated surface under <code>/admin</code>.
				</p>
			</div>
			<ol class="wf-flow-map">
				<li>Declare route reads with <code>@load</code> and <code>@live</code></li>
				<li>Let the generated static registry drive SSR</li>
				<li>Hydrate one browser replica with separate authority proof</li>
				<li>Read with generated <code>Todos.use()</code> / <code>BlobGames.use()</code></li>
				<li>Write through generated nested commands</li>
				<li>Elevated ops only via <code>$distributed/admin</code></li>
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
				<span class="wf-label">Result shapes</span>
				<h2>Causal vs Projected — pick per feature</h2>
				<p>
					Not every screen needs the same consistency story. This fixture shows both on purpose so
					you can feel the tradeoff instead of arguing in the abstract.
				</p>
			</div>
			<div class="wf-cards">
				<div class="wf-card wf-card-accent">
					<span class="wf-card-kicker">Projected</span>
					<h3>Blob — row commits with the command</h3>
					<p>
						<code>PreparedCommand&lt;Projected&lt;BlobGames&gt;&gt;</code> +
						<code>project(BLOB_GAMES).commit().projected()</code>. Map/score are in the
						mutation payload; the replica applies them before the call resolves. Revalidation may
						still race — fences keep your own write from rolling back under a lagging stamp.
					</p>
				</div>
				<div class="wf-card">
					<span class="wf-card-kicker">Causal + modeled projection</span>
					<h3>Todos — paint now, confirm later</h3>
					<p>
						Command returns a causal result. Its event set plus <code>state_preview!</code> safely
						specialize the same <code>projection!</code> program for the replica; actual emitted
						occurrences mint exact obligations for the active projector epoch. Unknown values
						revalidate, and history remains the source of truth.
					</p>
				</div>
				<div class="wf-card">
					<span class="wf-card-kicker">Causal + live</span>
					<h3>Chat — other people are the clock</h3>
					<p>
						Your post can show immediately; everyone else’s posts arrive over the
						<code>@live</code> companion. That open connection is how the room converges — not a
						poll loop.
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
				<li>Page load queries with that token (RLS-filtered rows)</li>
				<li>UI sends a typed command, not a free-form table update</li>
				<li>Handler: session → load/create → domain → stage</li>
				<li>Return Causal (projector later) or Projected (same transaction)</li>
				<li>Display names join from the identity directory</li>
				<li>Queries and @live rooms show the new world</li>
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
				<h2>One inventory in, complete clients out</h2>
				<p>
					The typed Rust Service inventory defines readable models, commands, permissions, results,
					effects, and confirmations. <code>dctl client-manifest</code> extracts a selected
					application surface without a database; <code>dctl client</code> combines it with
					co-located route reads. Drift fails a check instead of becoming a production surprise.
				</p>
			</div>
			<div class="wf-cards wf-cards-tight">
				<div class="wf-card">
					<h3>Application surfaces are capabilities</h3>
					<p>
						<code>distributed_client_surface</code> → <code>e2e-ui</code> (user + admin roles for
						the normal shell). <code>distributed_admin_client_surface</code> →
						<code>e2e-ui-admin</code>, consumed only by the nested admin layout.
					</p>
				</div>
				<div class="wf-card">
					<h3>Routes declare reads, Rust declares writes</h3>
					<p>
						Use <code>+page.graphql</code> with <code>@load</code> / <code>@live</code>. Generation
						emits operations, command tree, static route registry, and SvelteKit adapter.
					</p>
				</div>
				<div class="wf-card">
					<h3>After you change the contract</h3>
					<p>
						Run <code>make gen-client</code>, inspect and commit artifacts, let
						<code>make check-client</code> enforce drift. Durable design belongs in the Distributed
						GitKB; this fixture stays executable.
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

// Inside the nested admin tree only:
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
						Blob’s <code>+page.graphql</code> selects the owner join on load. Chat’s
						<code>author</code> relationship is on the read model — add it to the query when a route
						needs labels.
					</p>
				</div>
				<div class="wf-card">
					<h3>Roles in practice</h3>
					<p>
						alice and bob are normal users (their own todos). admin sees everyone and can
						force-archive via the elevated surface. Groups on the session map into engine roles.
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
						A local array “for convenience” plus the shared cache for the same list is how board and
						history disagree. Use one replica-backed operation view.
					</p>
				</div>
				<div class="wf-card">
					<h3>Hard refetch that yanks the optimistic UI</h3>
					<p>
						Blasting a full reload the instant a command returns can flash or erase the row you just
						painted. Let effects / Projected payload stand until the replica observes confirmation
						or a live frame — and respect causal fences on revalidation.
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
					<h3>Elevated ops in the user client</h3>
					<p>
						Do not smuggle force-archive into the normal <code>e2e-ui</code> surface “for
						convenience.” Dual surfaces exist so capability matches generation.
					</p>
				</div>
				<div class="wf-card">
					<h3>Hand-editing generated files</h3>
					<p>
						Change the real source (Rust inventory or co-located query), regenerate, commit. CI
						checks exist so drift does not wait for a human on-call.
					</p>
				</div>
				<div class="wf-card">
					<h3>Tokens in the wrong place on WebSockets</h3>
					<p>
						Browsers cannot set Authorization on the upgrade the way they do on HTTP. Put the token
						in the first connection message — never in a long-lived URL query string.
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
				<li>
					Thin command handler + register typed result (<code>Causal</code> or
					<code>Projected</code>) + effects/confirmations as needed
				</li>
				<li>Projector — or SurfaceDirectProjection for Projected models</li>
				<li>Permissions + which application surface exports the op</li>
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
				Sign in (alice / bob / admin · Password1!). Feel todos settle after a moment, blob answer in
				the Projected payload, chat fill from others, admin use a separate surface, session explain
				who you are.
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
