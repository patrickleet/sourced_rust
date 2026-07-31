import type { DemoWalkthrough } from './types';

export const todosWalkthrough: DemoWalkthrough = {
	id: 'todos',
	href: '/todos',
	title: 'Todos',
	kicker: 'Causal · owner RLS · modeled optimism',
	summary:
		'Personal work items prove the eventual path: domain rules stay plain Rust, commands return Causal, a projector fills the list, and the browser shows safe optimism from the same inventory.',
	tabs: [
		{
			id: 'domain',
			label: 'Domain',
			lede: 'Start with behavior you can unit-test without HTTP or SQL.',
			principle: 'Start with the domain, not the database.',
			samples: [
				{
					file: 'todo-domain · Todo::complete',
					caption: 'Public method enforces rules; private #[event] records history.',
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
			id: 'command',
			label: 'Command',
			lede: 'Handlers load history, apply the domain method, and commit. Owner never comes from the client body.',
			principle: 'Trust the signed-in person, not the request body.',
			samples: [
				{
					file: 'handlers/commands · create',
					code: `let owner = ctx.user_id()?.to_string(); // never from input
let repo = ctx.repo();
let mut todo = repo.create();
todo.create(&input.todo_id, &owner, &input.title)?;
let state = TodoState::from(&*todo);
repo.publish_events().commit(todo)?.causal(TodoCreatePayload {
  todo_id: state.todo_id,
  owner_id: state.owner_id,
  title: state.title,
  status: state.status,
})`
				},
				{
					file: 'service · typed_command preview',
					caption: 'Inventory declares emitted events + safe pre-dispatch preview.',
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
			id: 'projection',
			label: 'Projection',
			lede: 'Domain events land in a portable program; the service mounts it once. No dual-write from the UI.',
			principle: 'Commands change the world; tables are for reading.',
			samples: [
				{
					file: 'projections/todos.rs',
					code: `portable_handlers! {
  pub const TODOS: ProjectionDescriptor<EventualOnly> = {
    name: "project_todos", version: 1, epoch: "e2e-ui-todos-v2",
    model: Todos,
    apply save_todo {
      on_event TodoCreatedDomainEvent, /* … */ as "todo"
    },
    apply delete_todo {
      on_deleted TodoPurgedDomainEvent as "todo_id"
    }
  };
}`
				}
			]
		},
		{
			id: 'client',
			label: 'Client',
			lede: 'One co-located query + generated commands. Optimism and causal confirmations come from the artifact.',
			principle: 'One replica story for user data.',
			samples: [
				{
					file: 'routes/todos/+page.graphql',
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
					file: 'routes/todos/+page.svelte',
					code: `import { Todos, useCommands } from '$distributed';

const list = Todos.use();
const commands = useCommands();
const rows = $derived($list.complete ? $list.data.todos : []);

await commands.todo.create({ title });
await commands.todo.complete({ todo_id });
// Modeled optimism is already visible; causal obligation confirms later`
				}
			]
		},
		{
			id: 'authz',
			label: 'AuthZ',
			lede: 'Row filters live on the query model. Alice never lists Bob’s todos.',
			principle: 'Roles and surfaces are real.',
			samples: [
				{
					file: 'service · ModelPermissions',
					code: `.model::<Todos>(
  ModelPermissions::new()
    .grant(
      "user",
      read().all_columns().rows(
        col("owner_id").eq(claim("x-user-id")),
      ),
    )
    .grant("admin", read().all_columns()),
)`
				}
			]
		}
	]
};

export const chatWalkthrough: DemoWalkthrough = {
	id: 'chat',
	href: '/chat',
	title: 'Lobby chat',
	kicker: '@load @live · shared room · Causal post',
	summary:
		'One GraphQL document seeds SSR and continues live over WebSocket. Posts are commands; other people arrive as live frames — not a second chat stack.',
	tabs: [
		{
			id: 'query',
			label: 'Query',
			lede: 'Declare the read beside the route. @load hydrates HTML; @live is the same operation over WS.',
			principle: 'Register once, ship everywhere.',
			samples: [
				{
					file: 'routes/chat/+page.graphql',
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
				}
			]
		},
		{
			id: 'live',
			label: 'Live',
			lede: 'Root layout hydrates one replica. Token goes in connection_init — not the upgrade URL.',
			principle: 'One replica story for user data.',
			samples: [
				{
					file: 'routes/+layout.svelte',
					code: `const client = provideDistributed({
  session: pageData.session,
  browser,
  hydration: data.distributed,
  authority: data.distributedAuthority
});
// Navigation re-hydrates after the session fence`
				},
				{
					file: 'WS · connection_init',
					code: `{
  "type": "connection_init",
  "payload": {
    "authorization": "Bearer <access_token>",
    "x-roles": "user"
  }
}`
				}
			]
		},
		{
			id: 'command',
			label: 'Command',
			lede: 'Posting is a command. Author comes from session; body is the only client input.',
			principle: 'Trust the signed-in person, not the request body.',
			samples: [
				{
					file: 'routes/chat/+page.svelte',
					code: `const lobby = ChatMessages.use({ limit: PAGE_SIZE, offset: 0 });
const commands = useCommands();

await commands.chat.post({
  room_id: "lobby",
  body: draft.trim()
});
// Causal + projector → every subscriber’s @live frame`
				}
			]
		},
		{
			id: 'history',
			label: 'History',
			lede: 'Scroll-up loads the same operation with rising offset. Rows merge by message_id — one log model.',
			principle: 'Simplest DX is the goal.',
			samples: [
				{
					file: 'chat · history fill',
					code: `// Same ChatMessages.artifact, larger offset
const page = await chat.fetch({ limit: PAGE_SIZE, offset: historyOffset });
history = mergeHistoryPage(history, page);
// Live page stays at offset: 0 with @live`
				}
			]
		},
		{
			id: 'identity',
			label: 'Joins',
			lede: 'Display names come from auth_users joins — not copied onto every message aggregate.',
			principle: 'Know which side of the fence you are on.',
			samples: [
				{
					file: 'ChatMessages · belongs_to author',
					code: `// author_id = OIDC sub
// author { display_name } joins auth_users
// Fix missing people at Zitadel ingest/scrape — not in chat domain`
				}
			]
		}
	]
};

export const blobWalkthrough: DemoWalkthrough = {
	id: 'blob',
	href: '/blob',
	title: 'Blob game',
	kicker: 'Projected · same transaction · no dual-write',
	summary:
		'Each move returns Projected<BlobGames>. Aggregate, ledger, and board row commit together so the replica applies the map before the call resolves.',
	tabs: [
		{
			id: 'domain',
			label: 'Domain',
			lede: 'Movement rules and scoring live on a plain aggregate.',
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
  // …tile rules, score, visit marks…
  self.record_moved(dir)?;
  Ok(())
}`
				}
			]
		},
		{
			id: 'projected',
			label: 'Projected',
			lede: 'Strong consistency when lag would break the product. Placement selects the direct projection.',
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
			id: 'client',
			label: 'Client',
			lede: 'URL selects the active game; the board derives from one replica operation.',
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
					file: 'blob · move',
					code: `const receipt = await commands.blob.move({
  game_id,
  direction: 'up'
});
// consistency: "projected" — board hits the replica
// from the mutation payload before this resolves`
				}
			]
		},
		{
			id: 'rls',
			label: 'RLS',
			lede: 'You only list games you own (unless admin).',
			principle: 'Roles and surfaces are real.',
			samples: [
				{
					file: 'BlobGames permissions',
					code: `ModelPermissions::new()
  .grant("user", read().all_columns()
    .rows(col("owner_id").eq(claim("x-user-id"))))
  .grant("admin", read().all_columns())`
				}
			]
		}
	]
};

export const adminWalkthrough: DemoWalkthrough = {
	id: 'admin',
	href: '/admin',
	title: 'Admin surface',
	kicker: 'e2e-ui-admin · separate client · elevated ops',
	summary:
		'Elevated work is not hidden behind a boolean on the user tree. A second generated application surface and nested layout gate keep force-archive undiscoverable from $distributed.',
	tabs: [
		{
			id: 'surface',
			label: 'Surface',
			lede: 'Two clients, two inventories. User pages cannot import force_archive.',
			principle: 'Roles and surfaces are real.',
			samples: [
				{
					file: 'service · dual surfaces',
					code: `// e2e-ui — portable user contract
// e2e-ui-admin — elevated queries + force_archive
// Multi-role principals open a *named* application surface`
				},
				{
					file: 'admin/+layout.server.ts',
					code: `import { DISTRIBUTED_ROUTE_OPERATIONS } from '$distributed/admin';

const distributed = createDistributedSvelteKitServer({
  routes: DISTRIBUTED_ROUTE_OPERATIONS,
  getRole: (session) => {
    const role = engineRoleFromGroups(session?.user?.groups);
    if (!isAdminEngineRole(role)) error(403, 'Admin role required');
    return role;
  },
});`
				}
			]
		},
		{
			id: 'command',
			label: 'Command',
			lede: 'Elevated mutations only exist on the admin command tree.',
			principle: 'You keep the interesting code — scaffolding disappears.',
			samples: [
				{
					file: 'admin page',
					code: `import { AdminAllTodos, useCommands } from '$distributed/admin';

const list = AdminAllTodos.use();
const commands = useCommands();
await commands.todo.force_archive({ todo_id });
// Not present on the user command tree`
				}
			]
		},
		{
			id: 'read',
			label: 'Read all',
			lede: 'Admin list is all owners — still GraphQL RLS, not a secret REST dump.',
			principle: 'Know which side of the fence you are on.',
			samples: [
				{
					file: 'admin · AdminAllTodos',
					code: `// grant("admin", read().all_columns()) on Todos
// Same model, different privilege pack on e2e-ui-admin`
				}
			]
		}
	]
};

export const sessionWalkthrough: DemoWalkthrough = {
	id: 'session',
	href: '/session',
	title: 'Session',
	kicker: 'OIDC · groups → engine roles · tokens',
	summary:
		'Who am I to the API? Start here when GraphQL is empty or 401. Session holds access/refresh; groups map to engine roles used by surfaces and RLS.',
	tabs: [
		{
			id: 'oidc',
			label: 'OIDC',
			lede: 'Auth.js + Zitadel issue tokens; SSR and GraphQL use Bearer access tokens.',
			principle: 'Trust the signed-in person, not the request body.',
			samples: [
				{
					file: 'ui/src/auth.ts',
					code: `callbacks: {
  async jwt({ token, account }) {
    if (account) {
      token.accessToken = account.access_token;
      token.refreshToken = account.refresh_token;
      token.expiresAt = account.expires_at ?? …;
    }
    return token;
  },
  async session({ session, token }) {
    session.accessToken = token.accessToken;
    session.user.id = token.sub;
    session.user.groups = /* from IdP claims */;
    return session;
  }
}`
				}
			]
		},
		{
			id: 'roles',
			label: 'Roles',
			lede: 'IdP groups map to engine roles. Identity is a set (x-roles); execution opens a surface privilege pack.',
			principle: 'Roles and surfaces are real.',
			samples: [
				{
					file: 'lib/roles.ts',
					code: `// groups → engineRoleFromGroups
// admin principals often assert admin + user
// multi-role without a named surface fails closed`
				}
			]
		},
		{
			id: 'tokens',
			label: 'Tokens',
			lede: 'Inspect what the browser actually holds — access vs id vs refresh.',
			principle: 'Simplest DX is the goal.',
			samples: [
				{
					file: 'session · TokenInspector',
					code: `// Access token → Authorization: Bearer for GraphQL
// Refresh → silent renewal before expiry
// Id token → claims for UI display (not API authz)`
				}
			]
		}
	]
};

export const publicWalkthrough: DemoWalkthrough = {
	id: 'public',
	href: '/public',
	title: 'Public surface',
	kicker: 'anonymous · e2e-ui-public · empty identity',
	summary:
		'Unauthenticated open is not a fake “anonymous” role header. Empty identity + eligible anonymous on a named application surface.',
	tabs: [
		{
			id: 'surface',
			label: 'Surface',
			lede: 'Protocol client names the application surface and schema hash.',
			principle: 'Roles and surfaces are real.',
			samples: [
				{
					file: 'public · extensions.distributed.client',
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
				}
			]
		},
		{
			id: 'identity',
			label: 'Identity',
			lede: 'No session, no synthetic x-roles=anonymous injection. Privilege comes from the surface pack.',
			principle: 'Set-only identity; surface privilege for execution.',
			samples: [
				{
					file: 'resolve_execution_authority',
					code: `// empty asserted roles + eligible contains "anonymous"
// → open public surface privilege pack
// never mint a fake principal role header`
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
