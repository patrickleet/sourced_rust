import type { DemoWalkthrough } from './types';

/**
 * Tab order is browser-first teaching order for every demo:
 * 1. Query / live subscription
 * 2. Commands (optimistic cache vs atomic Projected)
 * 3. Command handlers (repo → aggregate → commit)
 * 4. Domain model (plain Rust + macros)
 * 5. Domain events + projections
 */

export const todosWalkthrough: DemoWalkthrough = {
	id: 'todos',
	href: '/todos',
	title: 'Todos',
	kicker: 'Browser → command → domain → projection',
	summary:
		'Start on the page: one @load query feeds the replica. Commands update a client-side cache optimistically; the server commits Causal and projectors catch up.',
	tabs: [
		{
			id: 'query',
			label: '1 · Query',
			lede: 'The browser reads through a co-located GraphQL document. @load seeds SSR; the generated operation binds the same document to the replica. Row filters are model RBAC — not ad-hoc WHERE in the UI.',
			principle: 'One replica story for user data.',
			samples: [
				{
					file: 'routes/todos/+page.graphql',
					caption: 'SSR loads this once; Todos.use() keeps watching the replica.',
					code: `query Todos @load {
  todos(order_by: [{ status: asc }, { todo_id: asc }]) {
    todo_id
    owner_id
    title
    status
  }
}`
				},
				{
					file: 'Todos · ModelPermissions (RBAC)',
					caption: 'Read grants: user sees own rows; admin sees all. Privilege pack applies at the engine.',
					code: `ModelPermissions::new()
  .grant(
    "user",
    read().all_columns().rows(
      col("owner_id").eq(claim("x-user-id")),
    ),
  )
  .grant("admin", read().all_columns())
// No anonymous grant — /todos requires sign-in`
				},
				{
					file: 'routes/todos/+page.svelte',
					caption: 'UI data comes from the client replica — not a hand-rolled store.',
					code: `import { Todos, useCommands } from '$distributed';

const list = Todos.use();
const rows = $derived($list.complete ? $list.data.todos : []);
// $list is the same cache SSR hydrated and commands update`
				}
			]
		},
		{
			id: 'commands',
			label: '2 · Commands',
			lede: 'Writes go through generated commands. Todos are Causal: the client applies a safe optimistic preview into the replica cache that feeds the UI, then confirms when the projection obligation completes. Command RBAC lists who may invoke each mutation.',
			principle: 'Let the Service declare how the UI catches up.',
			samples: [
				{
					file: 'routes/todos/+page.svelte',
					caption: 'Optimistic via client cache — no page-local setState surgery.',
					code: `const commands = useCommands();

await commands.todo.create({ title: text });
// Inventory preview already painted the row in the replica
await commands.todo.complete({ todo_id });
// Causal: optimistic status → projector confirms exact obligation`
				},
				{
					file: 'service · command roles (RBAC)',
					caption: 'User surface can create/complete; force_archive is admin-only (elevated surface).',
					code: `// app_roles = ["user", "admin"] on portable mutations
typed_command::<TodoCreateInput, Causal<…>>(…)
  .roles(app_roles)   // user + admin may create
  .emits(…)
  .preview(…);

typed_command::<TodoForceArchiveInput, Causal<…>>(…)
  .roles(["admin"])   // not on $distributed user tree`
				},
				{
					file: 'service · typed_command preview',
					caption: 'Safe fields only — known from input, defaults, or trusted claims.',
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
			id: 'handlers',
			label: '3 · Handlers',
			lede: 'Command handlers use the repository pattern: get or create the aggregate, call a domain method, commit. Todos choose eventual consistency (Causal + projector).',
			principle: 'Commands change the world; tables are for reading.',
			samples: [
				{
					file: 'handlers/commands · create',
					caption: 'repo → create → domain → publish_events → causal',
					code: `let owner = ctx.user_id()?.to_string(); // never from input
let repo = ctx.repo();
let mut todo = repo.create();
todo.create(&input.todo_id, &owner, &input.title)?;
let state = TodoState::from(&*todo);
repo.publish_events()
  .commit(todo)?
  .causal(TodoCreatePayload {
    todo_id: state.todo_id,
    owner_id: state.owner_id,
    title: state.title,
    status: state.status,
  })`
				},
				{
					file: 'handlers/commands · complete',
					code: `let mut todo = repo.get(&input.todo_id).await?
  .ok_or_else(|| HandlerError::NotFound(…))?;
todo.complete(&owner).map_err(rejected)?;
let state = TodoState::from(&*todo);
repo.publish_events().commit(todo)?.causal(…)`
				}
			]
		},
		{
			id: 'domain',
			label: '4 · Domain',
			lede: 'The model is a plain Rust struct with Distributed macros. Public methods enforce rules; private #[event] helpers record history. Unit-testable with no HTTP or SQL.',
			principle: 'Start with the domain, not the database.',
			samples: [
				{
					file: 'todo-domain · Todo::complete',
					code: `pub fn complete(&mut self, owner_id: &str) -> Result<(), TodoError> {
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
}`
				}
			]
		},
		{
			id: 'events',
			label: '5 · Events',
			lede: 'Domain methods emit events. Projections are event handlers that upsert (or delete) read-model rows. The UI never dual-writes the todos table.',
			principle: 'Know which side of the fence you are on.',
			samples: [
				{
					file: 'projections/todos.rs',
					caption: 'Domain events → portable projection program.',
					code: `portable_handlers! {
  pub const TODOS: ProjectionDescriptor<EventualOnly> = {
    name: "project_todos", version: 1, epoch: "e2e-ui-todos-v2",
    model: Todos,
    apply save_todo {
      on_event TodoCreatedDomainEvent,
              TodoCompletedDomainEvent,
              /* … */ as "todo"
    },
    apply delete_todo {
      on_deleted TodoPurgedDomainEvent as "todo_id"
    }
  };
}`
				},
				{
					file: 'handlers/events/project_todos.rs',
					code: `pub async fn handle(
  context: CausalProjectorContext,
  projection: ModeledProjection,
) -> Result<(), HandlerError> {
  projection.apply(TODOS, &context).await
}`
				}
			]
		}
	]
};

export const chatWalkthrough: DemoWalkthrough = {
	id: 'chat',
	href: '/chat',
	title: 'Lobby chat',
	kicker: 'Browser → live query → Causal post',
	summary:
		'Start with the document: @load seeds HTML and @live continues the same query over WebSocket. Posts are optimistic Causal commands into the shared replica.',
	tabs: [
		{
			id: 'query',
			label: '1 · Query / live',
			lede: 'One GraphQL operation is both the SSR seed and the live subscription. Newest page stays at offset 0 with @live; history uses the same op with rising offset. Read RBAC allows user, admin, and anonymous.',
			principle: 'Register once, ship everywhere.',
			samples: [
				{
					file: 'routes/chat/+page.graphql',
					caption: '@load for SSR · @live for WS frames',
					code: `query ChatMessages @load @live {
  chat_messages(
    where: { room_id: { _eq: "lobby" } }
    limit: 40
    offset: 0
    order_by: [{ created_at: desc }]
  ) {
    message_id
    room_id
    author_id
    body
    created_at
    author { display_name }
  }
}`
				},
				{
					file: 'ChatMessages · ModelPermissions (RBAC)',
					caption: 'Room-shared read for every role pack that includes this model.',
					code: `ModelPermissions::new()
  .grant("user", read().all_columns())
  .grant("admin", read().all_columns())
  .grant("anonymous", read().all_columns())
// Guests open e2e-ui-public (anonymous privilege);
// signed-in clients open e2e-ui (user privilege)`
				},
				{
					file: 'routes/chat/+page.svelte',
					code: `const lobby = ChatMessages.use({ limit: PAGE_SIZE, offset: 0 });
// Live page = replica subscription on the same document
const livePage = $derived(/* reverse $lobby.data.chat_messages */);`
				}
			]
		},
		{
			id: 'commands',
			label: '2 · Commands',
			lede: 'Post is a generated command for signed-in surfaces only. The client replica cache applies a modeled optimistic message so the UI updates immediately; Causal confirmation follows the projector.',
			principle: 'One replica story for user data.',
			samples: [
				{
					file: 'routes/chat/+page.svelte',
					caption: 'Optimistic via shared client cache that feeds every bound view.',
					code: `const commands = useCommands();

await commands.chat.post({
  room_id: "lobby",
  body: draft.trim()
});
// Your bubble appears from replica optimism;
// others receive the same row via @live after project`
				},
				{
					file: 'service · chat.post roles (RBAC)',
					caption: 'Mutation roles are app_roles (user + admin). Anonymous surface has no write inventory.',
					code: `typed_command::<ChatPostInput, Causal<ChatPostPayload>>(…)
  .roles(app_roles)  // ["user", "admin"]
  .emits(…);
// e2e-ui-public: read-only privilege pack — guests see
// “Sign in to post”, not a composer that 403s later`
				}
			]
		},
		{
			id: 'handlers',
			label: '3 · Handlers',
			lede: 'Handler loads or creates the chat aggregate through the repository, applies the domain post, commits Causal (eventual path).',
			principle: 'Trust the signed-in person, not the request body.',
			samples: [
				{
					file: 'handlers/commands · chat post',
					caption: 'Author from session — never “I am alice” in the body.',
					code: `let author = ctx.user_id()?.to_string();
let repo = ctx.repo();
let mut room = /* get/create lobby aggregate */;
room.post(&author, &input.body, …)?;
repo.publish_events().commit(room)?.causal(ChatPostPayload { … })`
				}
			]
		},
		{
			id: 'domain',
			label: '4 · Domain',
			lede: 'Chat domain is plain Rust: posting rules and event recording, testable without GraphQL.',
			principle: 'Start with the domain, not the database.',
			samples: [
				{
					file: 'chat-domain · post',
					code: `pub fn post(
  &mut self,
  author_id: &str,
  body: &str,
) -> Result<(), ChatError> {
  // validation, rate, room membership…
  self.record_posted(author_id, body)?;
  Ok(())
}

#[event("chat.message.posted", version = 1, domain)]
fn record_posted(&mut self, author_id: &str, body: &str) { … }`
				}
			]
		},
		{
			id: 'events',
			label: '5 · Events',
			lede: 'Domain events drive the chat_messages projection. ChangeHub wakes @live subscribers when rows land.',
			principle: 'Commands change the world; tables are for reading.',
			samples: [
				{
					file: 'projections · chat messages',
					code: `// on ChatMessagePostedDomainEvent → upsert chat_messages
// @live subscribers re-query / receive frames from the same model
// display names join auth_users — not copied onto the message aggregate`
				}
			]
		}
	]
};

export const blobWalkthrough: DemoWalkthrough = {
	id: 'blob',
	href: '/blob',
	title: 'Blob game',
	kicker: 'Browser → Projected (atomic) · no lag',
	summary:
		'Still start in the browser: one @load query owns the board. Moves return Projected — the replica applies the authoritative board from the mutation payload before the call resolves (atomic, not eventual optimism).',
	tabs: [
		{
			id: 'query',
			label: '1 · Query',
			lede: 'One operation lists games (and map JSON). URL selects which game is active; the board derives from the replica. Row RBAC scopes lists to the owner (unless admin).',
			principle: 'One replica story for user data.',
			samples: [
				{
					file: 'routes/blob/[[gameId]]/+page.graphql',
					code: `query BlobGames @load {
  blob_games(order_by: [{ game_id: asc }]) {
    game_id
    score
    map_json
    owner { user_id display_name }
  }
}`
				},
				{
					file: 'BlobGames · ModelPermissions (RBAC)',
					caption: 'Same claim pattern as todos — owner-scoped for user.',
					code: `ModelPermissions::new()
  .grant("user", read().all_columns()
    .rows(col("owner_id").eq(claim("x-user-id"))))
  .grant("admin", read().all_columns())
// No anonymous — /blob requires sign-in`
				},
				{
					file: 'blob · replica bind',
					code: `const list = BlobGames.use();
const games = $derived($list.complete ? $list.data.blob_games : []);
// Active board = games.find(g => g.game_id === routeGameId)`
				}
			]
		},
		{
			id: 'commands',
			label: '2 · Commands',
			lede: 'Moves are Projected commands. Unlike todos, the UI does not guess the next board — it applies atomic results from the server into the client cache that feeds the UI. Command roles match the portable surface.',
			principle: 'Let the Service declare how the UI catches up.',
			samples: [
				{
					file: 'blob · move',
					caption: 'consistency: "projected" — authoritative delta before await returns.',
					code: `const receipt = await commands.blob.move({
  game_id,
  direction: 'up'
});
// Replica already has the new map_json / score from the payload
// No dual-write; no “wait for projector” flash`
				},
				{
					file: 'service · blob command roles (RBAC)',
					caption: 'start / move / start_level are user+admin on e2e-ui.',
					code: `typed_command::<BlobMoveInput, Projected<BlobGames>>(…)
  .roles(app_roles);  // ["user", "admin"]
// Domain still enforces owner_id on the aggregate —
// RBAC is “who may call”; domain is “who may mutate this game”`
				}
			]
		},
		{
			id: 'handlers',
			label: '3 · Handlers',
			lede: 'Same repo pattern: get aggregate, domain move, commit — but commit returns Projected so aggregate, ledger, and query row share one transaction.',
			principle: 'Commands change the world; tables are for reading.',
			samples: [
				{
					file: 'handlers/commands/blob_move.rs',
					code: `let mut game = repo.get(&input.game_id).await?
  .ok_or_else(|| HandlerError::NotFound(input.game_id.clone()))?;
game.move_dir(&owner, dir).map_err(rejected)?;
// Placement-selected direct projection
repo.commit(game)?.projected()
// → PreparedCommand<Projected<BlobGames>>`
				}
			]
		},
		{
			id: 'domain',
			label: '4 · Domain',
			lede: 'Movement and scoring live on a plain Rust aggregate with #[event] history.',
			principle: 'Start with the domain, not the database.',
			samples: [
				{
					file: 'blob-domain · move_dir',
					code: `pub fn move_dir(
  &mut self,
  owner_id: &str,
  dir: Direction,
) -> Result<(), BlobError> {
  self.ensure_owner(owner_id)?;
  self.require_alive()?;
  // tile rules, score, visit marks…
  self.record_moved(dir)?;
  Ok(())
}`
				}
			]
		},
		{
			id: 'events',
			label: '5 · Events',
			lede: 'Domain events still exist for history. For blob, the direct projection path writes the read model in the same commit as the event — not a later eventual handler for the board.',
			principle: 'Know which side of the fence you are on.',
			samples: [
				{
					file: 'service · SurfaceDirectProjection',
					code: `fn blob_projection() -> SurfaceDirectProjection {
  SurfaceDirectProjection::new("project_blob")
    .modeled(catalog.resolve(BLOB_GAMES))
}
// typed_command::<…, Projected<BlobGames>>(…)
// Eventual projectors optional for side tables — board is atomic`
				}
			]
		}
	]
};

export const adminWalkthrough: DemoWalkthrough = {
	id: 'admin',
	href: '/admin',
	title: 'Admin surface',
	kicker: 'Browser → second client → elevated command',
	summary:
		'Start with the elevated query on e2e-ui-admin. Force-archive is a Causal command on that surface only — still optimistic cache on the admin replica, still repo → aggregate → commit on the server.',
	tabs: [
		{
			id: 'query',
			label: '1 · Query',
			lede: 'Admin list is a different generated client and route registry. Same GraphQL engine, different surface privilege — admin grant sees every owner’s todos.',
			principle: 'Roles and surfaces are real.',
			samples: [
				{
					file: 'admin · AdminAllTodos',
					code: `import { AdminAllTodos, useCommands } from '$distributed/admin';

const list = AdminAllTodos.use();
const rows = $derived($list.complete ? $list.data.todos : []);
// Nested layout provides a second distributed client`
				},
				{
					file: 'Todos · admin read grant (RBAC)',
					caption: 'e2e-ui-admin privilege pack uses admin grants (all rows).',
					code: `// ModelPermissions on Todos:
.grant("admin", read().all_columns())
// Portable e2e-ui uses schema privilege "user" (owner rows).
// Elevated e2e-ui-admin opens with admin privilege pack.`
				}
			]
		},
		{
			id: 'commands',
			label: '2 · Commands',
			lede: 'Elevated mutations only exist on the admin command tree. They still update the admin replica cache that feeds this page’s UI. Command roles are the second RBAC half.',
			principle: 'You keep the interesting code — scaffolding disappears.',
			samples: [
				{
					file: 'admin page',
					code: `const commands = useCommands(); // from $distributed/admin
await commands.todo.force_archive({ todo_id });
// Not importable from the user $distributed tree`
				},
				{
					file: 'service · force_archive roles (RBAC)',
					caption: 'Only admin may invoke; surface gate + typed_command roles.',
					code: `typed_command::<TodoForceArchiveInput, Causal<…>>(…)
  .roles(["admin"]);
// Layout: isAdminEngineRole before any GraphQL
// Artifact: force_archive absent from user command inventory`
				}
			]
		},
		{
			id: 'handlers',
			label: '3 · Handlers',
			lede: 'Handler still uses repo.get → domain force_archive → Causal commit. Authorization is role + surface, not a special HTTP path.',
			principle: 'Trust the signed-in person, not the request body.',
			samples: [
				{
					file: 'handlers · force_archive',
					code: `let mut todo = repo.get(&input.todo_id).await?…;
todo.force_archive(/* admin path */)?;
repo.publish_events().commit(todo)?.causal(…)`
				}
			]
		},
		{
			id: 'domain',
			label: '4 · Domain',
			lede: 'Same Todo aggregate — elevated methods live on the domain type, not in the GraphQL layer.',
			principle: 'Start with the domain, not the database.',
			samples: [
				{
					file: 'todo-domain · force_archive',
					code: `// Domain still owns “what does archive mean?”
// Handler only checks admin role / surface before calling it`
				}
			]
		},
		{
			id: 'events',
			label: '5 · Events',
			lede: 'Archive emits domain events into the same todos projection path — every surface’s replica converges on one query model.',
			principle: 'Register once, ship everywhere.',
			samples: [
				{
					file: 'projections/todos.rs',
					code: `// TodoArchivedDomainEvent → save_todo / status update
// Admin and user clients both read Todos — different RLS grants`
				}
			]
		}
	]
};

export const sessionWalkthrough: DemoWalkthrough = {
	id: 'session',
	href: '/session',
	title: 'Session',
	kicker: 'Browser identity → OIDC → engine roles',
	summary:
		'This page is the browser’s view of who you are. Tokens and groups become the Bearer session and x-roles set that every query and command above relies on.',
	tabs: [
		{
			id: 'query',
			label: '1 · Session UI',
			lede: 'No GraphQL list here — the “query” is the Auth.js session the layout already loaded. Groups map to engine roles; that principal drives RBAC on every other page.',
			principle: 'Trust the signed-in person, not the request body.',
			samples: [
				{
					file: 'routes/session/+page.svelte',
					code: `const session = $derived(data.session);
const user = $derived(session?.user);
const engineRole = $derived(
  engineRoleFromGroups(user?.groups)
);
// Access token → GraphQL Authorization on every other page`
				},
				{
					file: 'lib/roles.ts · groups → engine role (RBAC)',
					caption: 'UI + SSR map IdP groups to admin | user before GraphQL.',
					code: `export function engineRoleFromGroups(groups?: string[]) {
  if (groups?.includes("admin") || groups?.includes("admins"))
    return "admin";
  return "user";
}
// Multi-role tokens may assert admin+user in x-roles;
// surfaces pick which privilege pack executes`
				}
			]
		},
		{
			id: 'commands',
			label: '2 · Tokens',
			lede: 'Commands and queries do not invent identity. They carry the access token; the engine maps claims to x-user-id + x-roles (set-only). That set is the RBAC input for grants and .roles([…]).',
			principle: 'Roles and surfaces are real.',
			samples: [
				{
					file: 'layout · provideDistributed',
					code: `provideDistributed({
  session: pageData.session, // includes accessToken
  hydration: data.distributed,
  authority: data.distributedAuthority
});
// HTTP Bearer + WS connection_init share this source`
				},
				{
					file: 'identity · claim map (RBAC)',
					caption: 'OIDC groups/roles claims → allowlisted engine roles on the session.',
					code: `// OidcConfig claim_map.engine_roles = ["user", "admin"]
// role_claims: groups, roles, realm_access.roles, Zitadel project roles
// Session carries x-roles as a set — not a single primary role`
				}
			]
		},
		{
			id: 'handlers',
			label: '3 · Edge map',
			lede: 'OidcBearer validates JWT and injects session variables. Handlers then call ctx.user_id() / roles — repository pattern sits on top of that principal.',
			principle: 'Set-only identity; surface privilege for execution.',
			samples: [
				{
					file: 'service · OidcBearer',
					code: `// Bearer → validate → Session { x-user-id, x-roles }
// Multi-role principals open a named application surface
// Empty identity opens anonymous-eligible public surfaces only`
				}
			]
		},
		{
			id: 'domain',
			label: '4 · Claims',
			lede: 'Groups from the IdP map to engine roles in the browser and again in claim mapping. No domain aggregate for “session” — identity is transport.',
			principle: 'Simplest DX is the goal.',
			samples: [
				{
					file: 'lib/roles.ts · auth.ts',
					code: `// groups → engineRoleFromGroups
// Auth.js jwt/session callbacks store access + refresh
// Silent refresh before expiry when refresh_token present`
				}
			]
		},
		{
			id: 'events',
			label: '5 · Directory',
			lede: 'People still appear as auth_users via Zitadel ingest/scrape domain events — joins for chat author and blob owner, not a second display-name source.',
			principle: 'Know which side of the fence you are on.',
			samples: [
				{
					file: 'project_auth_user',
					code: `// zitadel.user.*.v1 → upsert auth_users
// Chat/blob join on OIDC sub == auth_users.user_id`
				}
			]
		}
	]
};

export const publicWalkthrough: DemoWalkthrough = {
	id: 'public',
	href: '/public',
	title: 'Public surface',
	kicker: 'Browser → anonymous surface · no session',
	summary:
		'Start with the request the browser would send: empty identity + named e2e-ui-public surface. No optimistic commands here — read-only anonymous privilege pack.',
	tabs: [
		{
			id: 'query',
			label: '1 · Query',
			lede: 'The teaching point is opening a surface with no session. Protocol extensions name e2e-ui-public; privilege pack is anonymous (chat + directory joins only).',
			principle: 'Roles and surfaces are real.',
			samples: [
				{
					file: 'public · GraphQL request',
					code: `{
  "query": "{ chat_messages(limit: 10) { message_id body } }",
  "extensions": {
    "distributed": {
      "client": {
        "surface": {
          "kind": "application",
          "name": "e2e-ui-public",
          "roles": ["anonymous"]
        },
        "schemaHash": "<from client-manifest>"
      }
    }
  }
}`
				},
				{
					file: 'anonymous privilege pack (RBAC)',
					caption: 'Only models granted to anonymous appear on this surface.',
					code: `// ChatMessages: grant("anonymous", read().all_columns())
// AuthUsers:    grant("anonymous", …)  // author display joins
// Todos / BlobGames: no anonymous grant → absent from public schema`
				}
			]
		},
		{
			id: 'commands',
			label: '2 · No writes',
			lede: 'Public surface is read-shaped. There is no optimistic command cache on this page — command RBAC simply does not expose mutations to anonymous.',
			principle: 'Simplest DX is the goal.',
			samples: [
				{
					file: 'public route',
					code: `// No useCommands() — unauthenticated lobby peek only
// Authed clients use e2e-ui / e2e-ui-admin instead`
				},
				{
					file: 'command inventory (RBAC)',
					caption: 'Public generated client has an empty command surface.',
					code: `// $distributed/public GeneratedCommands = Record<never, never>
// chat.post.roles(app_roles) never appears here —
// fail closed by omission, not by hoping the UI forgets to call it`
				}
			]
		},
		{
			id: 'handlers',
			label: '3 · Authority',
			lede: 'resolve_execution_authority: empty asserted roles + eligible anonymous → privilege pack for e2e-ui-public. No synthetic x-roles=anonymous header.',
			principle: 'Set-only identity; surface privilege for execution.',
			samples: [
				{
					file: 'engine · resolve_execution_authority',
					code: `// asserted.is_empty() && eligible contains "anonymous"
// → open application surface privilege
// never mint a fake principal role`
				}
			]
		},
		{
			id: 'domain',
			label: '4 · Chat model',
			lede: 'Reads still hit the same chat_messages query model and domain history that authenticated clients use — only the surface privilege differs.',
			principle: 'Register once, ship everywhere.',
			samples: [
				{
					file: 'ChatMessages · RLS',
					code: `// anonymous grant: room-shared lobby read
// same projection as signed-in chat`
				}
			]
		},
		{
			id: 'events',
			label: '5 · Events',
			lede: 'Messages still arrive from chat domain events + projection. Public clients only see what anonymous RLS allows.',
			principle: 'Commands change the world; tables are for reading.',
			samples: [
				{
					file: 'chat projection',
					code: `// ChatMessagePostedDomainEvent → chat_messages
// Public surface cannot post — only observe`
				}
			]
		}
	]
};

export const allWalkthroughs: DemoWalkthrough[] = [
	todosWalkthrough,
	chatWalkthrough,
	blobWalkthrough,
	adminWalkthrough,
	sessionWalkthrough,
	publicWalkthrough
];

export function walkthroughById(id: string): DemoWalkthrough | undefined {
	return allWalkthroughs.find((d) => d.id === id);
}
