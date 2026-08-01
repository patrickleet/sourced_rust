import type { DemoWalkthrough } from './types';

/**
 * Tab order is browser-first teaching order for every demo:
 * 1. Query / live subscription (+ ReadModel shape + read RBAC)
 * 2. Commands (optimistic cache vs atomic Projected) (+ write RBAC)
 * 3. Command handlers (repo → aggregate → commit)
 * 4. Domain model (plain Rust + macros)
 * 5. Domain events + projections
 *
 * Samples should be real, pasteable shapes from the fixture — not comment-only stubs.
 * Query tabs should include the read model definition (#[derive(ReadModel)] struct), not only
 * permissions snippets or GraphQL selection.
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
			lede: 'The browser reads through a co-located GraphQL document over a declared read model. @load seeds SSR; the generated operation binds the same document to the replica. Shape and row filters live on the model — not ad-hoc WHERE in the UI.',
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
					file: 'readmodels/models/todos.rs · Todos',
					caption: 'Query-oriented row shape. Plural name infers table `todos`; belongs_to joins the directory.',
					code: `#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ReadModel)]
#[readmodel(primary_key = ["todo_id"])]
pub struct Todos {
  #[readmodel(id)]
  pub todo_id: String,
  pub owner_id: String,
  pub title: String,
  /// open | completed | archived
  pub status: String,
  pub assignee_id: Option<String>,
  #[readmodel(belongs_to = "AuthUsers", foreign_key = "owner_id")]
  pub owner: Option<AuthUsers>,
}`
				},
				{
					file: 'readmodels/models/todos.rs · permissions',
					caption: 'Read RBAC: user sees own rows; admin sees all.',
					code: `impl Todos {
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
}`
				},
				{
					file: 'routes/todos/+page.svelte',
					caption: 'UI data comes from the client replica — not a hand-rolled store.',
					code: `import { Todos, useCommands } from '$distributed';

const query = Todos.use();
const todos = $derived($query.complete ? $query.data.todos : []);`
				}
			]
		},
		{
			id: 'commands',
			label: '2 · Commands',
			lede: 'Writes go through generated commands. Todos are Causal: the client applies a safe optimistic preview into the replica cache that feeds the UI, then confirms when the projection obligation completes.',
			principle: 'Let the Service declare how the UI catches up.',
			samples: [
				{
					file: 'routes/todos/+page.svelte',
					caption: 'Optimistic via client cache — no page-local setState surgery.',
					code: `const commands = useCommands();

await commands.todo.create({ title: text });
await commands.todo.complete({ todo_id });`
				},
				{
					file: 'service.rs · todos_create (roles + preview)',
					caption: 'Write RBAC on the inventory; owner is a trusted claim, not input.',
					code: `typed_command::<TodoCreateInput, Causal<TodoCreatePayload>>(
  todo_create::COMMAND,
)
.field_name("todos_create")
.roles(app_roles) // ["user", "admin"]
.input_defaults(command_input_defaults! {
  input: TodoCreateInput;
  default input.todo_id = uuid_v7();
})
.emits(events![TodoCreatedDomainEvent])
.applies(state_preview! {
  TodoCreatedDomainEvent => TodoState {
    todo_id: generated.todo_id,
    owner_id: trusted("x-user-id", "string"),
    title: input.title,
    status: "open",
    assignee_id: null,
  }
})`
				},
				{
					file: 'service.rs · todos_force_archive roles',
					caption: 'Elevated mutation: admin only; not on the user client tree.',
					code: `typed_command::<TodoForceArchiveInput, Causal<TodoForceArchivePayload>>(
  todo_force_archive::COMMAND,
)
.field_name("todos_force_archive")
.roles(["admin"])
.emits(events![TodoForceArchivedDomainEvent])`
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
					file: 'handlers/commands/todo_create.rs',
					caption: 'repo → create → domain → publish_events → causal',
					code: `pub async fn handle(
  ctx: &CausalCommandContext<'_, Todo>,
  input: TodoCreateInput,
) -> Result<PreparedCommand<Causal<TodoCreatePayload>>, HandlerError> {
  let owner = ctx.user_id()?.to_string();
  let repo = ctx.repo();
  let mut todo = repo.create();
  todo.create(&input.todo_id, &owner, &input.title)
    .map_err(rejected)?;
  let state = TodoState::from(&*todo);
  repo.publish_events().commit(todo)?.causal(TodoCreatePayload {
    todo_id: state.todo_id,
    owner_id: state.owner_id,
    title: state.title,
    status: state.status,
  })
}`
				},
				{
					file: 'handlers/commands/todo_complete.rs',
					code: `let mut todo = repo
  .get(&input.todo_id)
  .await?
  .ok_or_else(|| HandlerError::NotFound(input.todo_id.clone()))?;
todo.complete(&owner).map_err(rejected)?;
let state = TodoState::from(&*todo);
repo.publish_events().commit(todo)?.causal(TodoStatusPayload {
  todo_id: state.todo_id,
  status: state.status,
})`
				}
			]
		},
		{
			id: 'domain',
			label: '4 · Domain',
			lede: 'The write model is a plain Rust aggregate — fields are the consistency boundary. Public methods enforce rules; private #[event] helpers record history. Unit-testable with no HTTP or SQL.',
			principle: 'Start with the domain, not the database.',
			samples: [
				{
					file: 'todo-domain · Todo',
					caption: 'Aggregate shape — the consistency boundary.',
					code: `#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Todo {
  pub entity: Entity,
  pub todo_id: String,
  pub owner_id: String,
  pub title: String,
  pub status: TodoStatus, // open | completed | archived
  pub assignee_id: Option<String>,
  purged: bool,
  snapshot_generation: u64,
}

#[sourced(
  entity,
  events = "TodoEvent",
  aggregate_type = "todo",
  domain_state = TodoState,
)]
impl Todo { /* create, complete, rename, archive, … */ }`
				},
				{
					file: 'todo-domain · Todo::complete',
					caption: 'One command path: validate → record domain event.',
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
  self.advance_snapshot_generation();
}`
				}
			]
		},
		{
			id: 'events',
			label: '5 · Events',
			lede: 'Domain methods emit events. Projections map those events onto syntax-only GraphQL mutations that become MutationProgram IR — upsert or delete read-model rows. The UI never dual-writes the todos table.',
			principle: 'Know which side of the fence you are on.',
			samples: [
				{
					file: 'projections/todos.rs',
					caption: 'Domain events → projection arms that name mutation programs.',
					code: `projection! {
  pub const TODOS: ProjectionDescriptor<EventualOnly> = {
    name: "project_todos",
    version: 1,
    epoch: "e2e-ui-todos-v2",
    model: Todos,
    on {
      events: [
        TodoCreatedDomainEvent,
        TodoRenamedDomainEvent,
        TodoCompletedDomainEvent,
        TodoReopenedDomainEvent,
        TodoReassignedDomainEvent,
        TodoArchivedDomainEvent,
        TodoForceArchivedDomainEvent,
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
}`
				},
				{
					file: 'projections/mutations/save_todo.mutation.graphql',
					caption: 'Not a public schema field — compiles to MutationProgram IR for the projector.',
					code: `# Syntax-only read-model mutation → MutationProgram IR.
mutation SaveTodo {
  upsert_Todos(object: $input.todo)
}`
				},
				{
					file: 'projections/mutations/delete_todo.mutation.graphql',
					caption: 'Purge path: delete by primary key from the event aggregate id.',
					code: `# Syntax-only read-model mutation → MutationProgram IR.
mutation DeleteTodo {
  delete_Todos_by_pk(todo_id: $input.todo_id)
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
		'Start with the document: @load seeds HTML and @live continues the same query over WebSocket. Posts are optimistic Causal commands into the shared replica. Guests read via e2e-ui-public.',
	tabs: [
		{
			id: 'query',
			label: '1 · Query / live',
			lede: 'One GraphQL operation is both the SSR seed and the live subscription over a declared ChatMessages read model. Read RBAC allows user, admin, and anonymous (guests open e2e-ui-public).',
			principle: 'Register once, ship everywhere.',
			samples: [
				{
					file: 'routes/chat/+page.graphql',
					caption: '@load for SSR · @live for WS frames',
					code: `query ChatMessages($limit: Int!, $offset: Int!) @load @live {
  chat_messages(
    where: { room_id: { _eq: "lobby" } }
    limit: $limit
    offset: $offset
    order_by: [{ created_at: desc }]
  ) {
    message_id
    room_id
    author_id
    body
    created_at
    author { user_id display_name email }
  }
}`
				},
				{
					file: 'readmodels/models/chat_messages.rs · ChatMessages',
					caption: 'Insert-shaped row; author is a belongs_to join onto AuthUsers.',
					code: `#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ReadModel)]
#[readmodel(primary_key = ["message_id"])]
pub struct ChatMessages {
  #[readmodel(id)]
  pub message_id: String,
  pub room_id: String,
  pub author_id: String,
  pub body: String,
  pub created_at: String,
  #[readmodel(belongs_to = "AuthUsers", foreign_key = "author_id")]
  pub author: Option<AuthUsers>,
}`
				},
				{
					file: 'readmodels/models/chat_messages.rs · permissions',
					caption: 'Room-shared read for user, admin, and anonymous.',
					code: `impl ChatMessages {
  pub fn permissions() -> ModelPermissions<Self> {
    ModelPermissions::new()
      .grant("user", read().all_columns())
      .grant("admin", read().all_columns())
      .grant("anonymous", read().all_columns())
  }
}`
				},
				{
					file: 'routes/chat/+page.svelte',
					code: `const lobby = ChatMessages.use({ limit: PAGE_SIZE, offset: 0 });
const livePage = $derived.by(() => {
  const pageMessages = Array.isArray($lobby.data?.chat_messages)
    ? $lobby.data.chat_messages
    : [];
  return [...pageMessages].reverse();
});`
				}
			]
		},
		{
			id: 'commands',
			label: '2 · Commands',
			lede: 'Post is a generated command for signed-in surfaces only. The client replica cache applies a modeled optimistic message; Causal confirmation follows the projector.',
			principle: 'One replica story for user data.',
			samples: [
				{
					file: 'routes/chat/+page.svelte',
					caption: 'Optimistic via shared client cache that feeds every bound view.',
					code: `const commands = useCommands();
const receipt = await commands.chat.post({
  message_id,
  body,
  room_id: LOBBY_ROOM,
  created_at: String(now),
});
if (receipt.projected !== undefined) {
  await receipt.projected;
}`
				},
				{
					file: 'service.rs · chat_messages_post roles',
					caption: 'Write RBAC: user + admin only. Public client has zero commands.',
					code: `typed_command::<ChatPostInput, Causal<ChatPostPayload>>(
  chat_post::COMMAND,
)
.field_name("chat_messages_post")
.roles(app_roles) // ["user", "admin"]
.emits(events![ChatMessagePostedDomainEvent])`
				},
				{
					file: 'generated/public/commands.ts',
					caption: 'e2e-ui-public inventory is intentionally empty.',
					code: `export const COMMAND_ARTIFACTS = [] as const;
export type GeneratedCommands = Readonly<Record<never, never>>;`
				}
			]
		},
		{
			id: 'handlers',
			label: '3 · Handlers',
			lede: 'Handler creates the chat aggregate through the repository, applies the domain post, commits Causal (eventual path). Author is always the session principal.',
			principle: 'Trust the signed-in person, not the request body.',
			samples: [
				{
					file: 'handlers/commands/chat_post.rs',
					code: `pub async fn handle(
  ctx: &CausalCommandContext<'_, ChatMessage>,
  input: ChatPostInput,
) -> Result<PreparedCommand<Causal<ChatPostPayload>>, HandlerError> {
  let author = ctx.user_id()?.to_string();
  let created_at = canonical_near_unix_millis(&input.created_at)?;
  let repo = ctx.repo();

  if repo.get(&input.message_id).await?.is_some() {
    return Err(HandlerError::Rejected(format!(
      "message {} already exists",
      input.message_id
    )));
  }

  let mut msg = repo.create();
  msg.post(
    &input.message_id,
    &input.room_id,
    &author,
    &input.body,
    &created_at,
  )
  .map_err(rejected)?;

  let state = ChatMessageState::from(&*msg);
  repo.publish_events().commit(msg)?.causal(ChatPostPayload {
    message_id: state.message_id,
    room_id: state.room_id,
    author_id: state.author_id,
    body: state.body,
    created_at: state.created_at,
  })
}`
				}
			]
		},
		{
			id: 'domain',
			label: '4 · Domain',
			lede: 'Chat domain is a plain Rust aggregate — one message is one consistency boundary. Public methods enforce rules; private #[event] helpers record history. No GraphQL in the model.',
			principle: 'Start with the domain, not the database.',
			samples: [
				{
					file: 'chat-domain · ChatMessage',
					caption: 'Aggregate shape — the consistency boundary.',
					code: `#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatMessage {
  pub entity: Entity,
  pub message_id: String,
  pub room_id: String,
  pub author_id: String,
  pub body: String,
  /// RFC3339 timestamp (string for portable projections / SQLite text).
  pub created_at: String,
  snapshot_delivery_generation: u64,
}

#[sourced(
  entity,
  events = "ChatMessageEvent",
  aggregate_type = "chat_message",
  domain_state = ChatMessageState,
)]
impl ChatMessage { /* post, … */ }`
				},
				{
					file: 'chat-domain · ChatMessage::post',
					caption: 'Validate, then record the domain event that becomes history.',
					code: `pub fn post(
  &mut self,
  message_id: impl Into<String>,
  room_id: impl Into<String>,
  author_id: impl Into<String>,
  body: impl Into<String>,
  created_at: impl Into<String>,
) -> Result<(), ChatError> {
  if self.is_posted() {
    return Err(ChatError::AlreadyExists);
  }
  let body = body.into();
  let body = body.trim();
  if body.is_empty() {
    return Err(ChatError::EmptyBody);
  }
  self.record_posted(
    message_id.into(),
    room_id.into(),
    author_id.into(),
    body.to_string(),
    created_at.into(),
  )?;
  Ok(())
}

#[event("chat_message.posted", version = 1, domain)]
fn record_posted(
  &mut self,
  message_id: String,
  room_id: String,
  author_id: String,
  body: String,
  created_at: String,
) {
  self.entity.set_id(&message_id);
  self.message_id = message_id;
  self.room_id = room_id;
  self.author_id = author_id;
  self.body = body;
  self.created_at = created_at;
}`
				}
			]
		},
		{
			id: 'events',
			label: '5 · Events',
			lede: 'Domain events drive the chat_messages projection via a named GraphQL mutation program. ChangeHub wakes @live subscribers when rows land.',
			principle: 'Commands change the world; tables are for reading.',
			samples: [
				{
					file: 'projections/chat.rs',
					code: `projection! {
  pub const CHAT_MESSAGES: ProjectionDescriptor<DirectCandidate> = {
    name: "project_chat_messages",
    version: 1,
    epoch: "e2e-ui-chat-v2",
    model: ChatMessages,
    on {
      events: [ChatMessagePostedDomainEvent],
      mutation: save_chat_message,
      input: { message: body },
    },
  };
}`
				},
				{
					file: 'projections/mutations/save_chat_message.mutation.graphql',
					caption: 'Syntax-only upsert — not a browser-facing mutation.',
					code: `# Syntax-only read-model mutation → MutationProgram IR.
mutation SaveChatMessage {
  upsert_ChatMessages(object: $input.message)
}`
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
			lede: 'One operation lists games (and map JSON) from the BlobGames read model. URL selects which game is active; the board derives from the replica. Row RBAC scopes lists to the owner (unless admin).',
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
					file: 'readmodels/models/blob_games.rs · BlobGames',
					caption: 'Query-oriented board row; map_json is the serialized level; owner joins AuthUsers.',
					code: `#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ReadModel)]
#[readmodel(primary_key = ["game_id"])]
pub struct BlobGames {
  #[readmodel(id)]
  pub game_id: String,
  pub owner_id: String,
  pub score: i64,
  pub player_dead: bool,
  pub current_level: i64,
  pub current_level_completed: bool,
  pub map_json: String,
  /// active | dead | level_complete
  pub status: String,
  #[readmodel(belongs_to = "AuthUsers", foreign_key = "owner_id")]
  pub owner: Option<AuthUsers>,
}`
				},
				{
					file: 'readmodels/models/blob_games.rs · permissions',
					caption: 'Same claim pattern as todos — owner-scoped for user.',
					code: `impl BlobGames {
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
}`
				},
				{
					file: 'routes/blob/[[gameId]]/+page.svelte',
					code: `const query = BlobGames.use();
const games = $derived(
  $query.complete ? $query.data.blob_games : []
);`
				}
			]
		},
		{
			id: 'commands',
			label: '2 · Commands',
			lede: 'Moves are Projected commands. Unlike todos, the UI does not guess the next board — it applies atomic results from the server into the client cache that feeds the UI.',
			principle: 'Let the Service declare how the UI catches up.',
			samples: [
				{
					file: 'routes/blob/[[gameId]]/+page.svelte',
					caption: 'consistency: "projected" — authoritative delta before await returns.',
					code: `const receipt = await commands.blob.move({
  game_id,
  direction: 'up',
});`
				},
				{
					file: 'service.rs · blob.move roles',
					caption: 'Write RBAC: user + admin on the portable surface.',
					code: `typed_command::<BlobMoveInput, Projected<BlobGames>>(
  blob_move::COMMAND,
)
.field_name("blob_games_move")
.roles(app_roles) // ["user", "admin"]`
				}
			]
		},
		{
			id: 'handlers',
			label: '3 · Handlers',
			lede: 'Same repo pattern: get aggregate, domain move, commit — but the row is staged in-handler and commit returns Projected so aggregate, ledger, and query row share one transaction.',
			principle: 'Commands change the world; tables are for reading.',
			samples: [
				{
					file: 'handlers/commands/blob_move.rs',
					code: `pub async fn handle(
  ctx: &CausalCommandContext<'_, BlobGame>,
  input: BlobMoveInput,
) -> Result<PreparedCommand<Projected<BlobGames>>, HandlerError> {
  let owner = ctx.user_id()?.to_string();
  let dir = Direction::parse(&input.direction).ok_or_else(|| {
    HandlerError::Rejected(format!(
      "invalid direction \`{}\` (use up|down|left|right)",
      input.direction
    ))
  })?;

  let repo = ctx.repo();
  let mut game = repo
    .get(&input.game_id)
    .await?
    .ok_or_else(|| HandlerError::NotFound(input.game_id.clone()))?;
  game.move_dir(&owner, dir).map_err(rejected)?;

  let row = save_blob_game()
    .from_state(&BlobGameState::from(&*game))
    .map_err(|error| HandlerError::Other(Box::new(error)))?;
  repo.readmodel(row)
    .publish_events()
    .commit(game)?
    .projected()
}`
				}
			]
		},
		{
			id: 'domain',
			label: '4 · Domain',
			lede: 'The game is a plain Rust aggregate — score, map, and level live on the write model. Public methods enforce rules; private #[event] helpers record history.',
			principle: 'Start with the domain, not the database.',
			samples: [
				{
					file: 'blob-domain · BlobGame',
					caption: 'Aggregate shape — the consistency boundary.',
					code: `#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BlobGame {
  pub entity: Entity,
  pub game_id: String,
  pub owner_id: String,
  pub score: i64,
  pub player_dead: bool,
  /// 0 = no level yet; 1+ = active level index.
  pub current_level: i64,
  pub current_level_completed: bool,
  /// Current level map only.
  pub map: Vec<Vec<u8>>,
}

#[sourced(
  entity,
  events = "BlobGameEvent",
  aggregate_type = "blob",
  domain_state = BlobGameState,
)]
impl BlobGame { /* start, move_dir, … */ }`
				},
				{
					file: 'blob-domain · BlobGame::move_dir (event)',
					caption: 'After rules and tile sim, the move is one domain event.',
					code: `// … ensure_owner, bounds, tile simulation → score / dead / map …

self.record_moved(
  score,
  player_dead,
  level_complete,
  next_map,
  direction.as_str().to_string(),
)?;

#[event("blob.moved", version = 1, domain)]
fn record_moved(
  &mut self,
  score: i64,
  player_dead: bool,
  current_level_completed: bool,
  map: Vec<Vec<u8>>,
  _direction: String,
) {
  self.score = score;
  self.player_dead = player_dead;
  self.current_level_completed = current_level_completed;
  self.map = map;
}`
				}
			]
		},
		{
			id: 'events',
			label: '5 · Events',
			lede: 'Domain events still exist for history. For blob, the same save_blob_game mutation program can run direct (same commit as the event) — not only as a later eventual handler.',
			principle: 'Know which side of the fence you are on.',
			samples: [
				{
					file: 'projections/blob.rs',
					code: `projection! {
  pub const BLOB_GAMES: ProjectionDescriptor<DirectCandidate> = {
    name: "project_blob",
    version: 1,
    epoch: "e2e-ui-blob-v2",
    model: BlobGames,
    on {
      events: [
        BlobInitializedDomainEvent,
        BlobLevelStartedDomainEvent,
        BlobStartedDomainEvent,
        BlobMovedDomainEvent,
      ],
      mutation: save_blob_game,
      input: { game: body },
    },
  };
}`
				},
				{
					file: 'projections/mutations/save_blob_game.mutation.graphql',
					caption: 'Syntax-only upsert used by direct and eventual projection paths.',
					code: `# Syntax-only read-model mutation → MutationProgram IR.
mutation SaveBlobGame {
  upsert_BlobGames(object: $input.game)
}`
				},
				{
					file: 'service.rs · projection binding',
					code: `let blob_binding = ProjectionBinding::materialize_direct(
  BLOB_GAMES.direct(),
  source(),
  owner("project_blob"),
  /* … */,
  vec![projection_output::<BlobGames>()],
  /* … */,
)?;
// Handler stages the row via readmodel(row).commit()?.projected()`
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
			lede: 'Admin list is a different generated client and route registry over the same Todos read model. Same GraphQL engine, different surface privilege — admin grant sees every owner’s todos.',
			principle: 'Roles and surfaces are real.',
			samples: [
				{
					file: 'routes/admin/+page.svelte',
					code: `import { AdminAllTodos, useCommands } from '$distributed/admin';

const query = AdminAllTodos.use();
const commands = useCommands();
const todos = $derived($query.complete ? $query.data.todos : []);`
				},
				{
					file: 'readmodels/models/todos.rs · Todos',
					caption: 'Same query model as /todos — elevated surface, not a second table.',
					code: `#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ReadModel)]
#[readmodel(primary_key = ["todo_id"])]
pub struct Todos {
  #[readmodel(id)]
  pub todo_id: String,
  pub owner_id: String,
  pub title: String,
  pub status: String,
  pub assignee_id: Option<String>,
  #[readmodel(belongs_to = "AuthUsers", foreign_key = "owner_id")]
  pub owner: Option<AuthUsers>,
}`
				},
				{
					file: 'readmodels/models/todos.rs · admin grant',
					caption: 'e2e-ui-admin privilege pack uses admin grants (all rows).',
					code: `impl Todos {
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
}`
				},
				{
					file: 'service.rs · dual surfaces',
					code: `.client_application_surface_with_schema_roles(
  "e2e-ui",
  ["admin", "user"], // eligible
  ["user"],          // portable privilege pack
)
.client_application_surface("e2e-ui-admin", ["admin"])
.client_application_surface("e2e-ui-public", ["anonymous"])`
				}
			]
		},
		{
			id: 'commands',
			label: '2 · Commands',
			lede: 'Elevated mutations only exist on the admin command tree. They still update the admin replica cache that feeds this page’s UI.',
			principle: 'You keep the interesting code — scaffolding disappears.',
			samples: [
				{
					file: 'routes/admin/+page.svelte',
					code: `await commands.todo.force_archive({ todo_id });`
				},
				{
					file: 'service.rs · todos_force_archive',
					caption: 'Write RBAC: admin only; absent from user client inventory.',
					code: `typed_command::<
  TodoForceArchiveInput,
  Causal<TodoForceArchivePayload>,
>(todo_force_archive::COMMAND)
.field_name("todos_force_archive")
.roles(["admin"])
.emits(events![TodoForceArchivedDomainEvent])
.applies(state_preview! {
  TodoForceArchivedDomainEvent => TodoState {
    todo_id: input.todo_id,
    status: "archived",
    ..unknown
  }
})`
				},
				{
					file: 'routes/admin/+layout.server.ts',
					caption: 'Surface gate before any GraphQL.',
					code: `getRole: (session) => {
  const role = engineRoleFromGroups(session?.user?.groups);
  if (!isAdminEngineRole(role)) {
    error(403, 'Admin role required — sign in as admin');
  }
  return role;
}`
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
					file: 'handlers/commands/todo_force_archive.rs',
					code: `pub async fn handle(
  ctx: &CausalCommandContext<'_, Todo>,
  input: TodoForceArchiveInput,
) -> Result<PreparedCommand<Causal<TodoForceArchivePayload>>, HandlerError> {
  let admin = ctx.user_id()?.to_string();
  let repo = ctx.repo();
  let mut todo = repo
    .get(&input.todo_id)
    .await?
    .ok_or_else(|| HandlerError::NotFound(input.todo_id.clone()))?;

  todo.force_archive().map_err(rejected)?;
  let state = TodoState::from(&*todo);
  repo.publish_events()
    .commit(todo)?
    .causal(TodoForceArchivePayload {
      todo_id: state.todo_id,
      owner_id: state.owner_id,
      status: state.status,
      archived_by: admin,
    })
}`
				}
			]
		},
		{
			id: 'domain',
			label: '4 · Domain',
			lede: 'Same Todo aggregate as /todos — elevated methods live on the domain type, not in the GraphQL layer. One write model, many surfaces.',
			principle: 'Start with the domain, not the database.',
			samples: [
				{
					file: 'todo-domain · Todo',
					caption: 'Same aggregate shape as the user surface.',
					code: `#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Todo {
  pub entity: Entity,
  pub todo_id: String,
  pub owner_id: String,
  pub title: String,
  pub status: TodoStatus,
  pub assignee_id: Option<String>,
  purged: bool,
  snapshot_generation: u64,
}

#[sourced(
  entity,
  events = "TodoEvent",
  aggregate_type = "todo",
  domain_state = TodoState,
)]
impl Todo { /* create, complete, force_archive, … */ }`
				},
				{
					file: 'todo-domain · Todo::force_archive',
					caption: 'Admin path is still a domain event on the same aggregate.',
					code: `/// Administrator intervention — separate event from owner archival.
pub fn force_archive(&mut self) -> Result<(), TodoError> {
  if !self.is_created() {
    return Err(TodoError::NotCreated);
  }
  self.record_force_archived()?;
  Ok(())
}

#[event("todo.force_archived", version = 1, domain)]
fn record_force_archived(&mut self) {
  self.status = TodoStatus::Archived;
  self.advance_snapshot_generation();
}`
				}
			]
		},
		{
			id: 'events',
			label: '5 · Events',
			lede: 'Force-archive emits TodoForceArchivedDomainEvent into the same todos projection path (and same save_todo mutation) as owner archive — every surface’s replica converges on one query model.',
			principle: 'Register once, ship everywhere.',
			samples: [
				{
					file: 'projections/todos.rs · force archive arm',
					code: `projection! {
  pub const TODOS: ProjectionDescriptor<EventualOnly> = {
    name: "project_todos",
    version: 1,
    epoch: "e2e-ui-todos-v2",
    model: Todos,
    on {
      events: [
        TodoCreatedDomainEvent,
        /* … */
        TodoArchivedDomainEvent,
        TodoForceArchivedDomainEvent, // admin path
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
}`
				},
				{
					file: 'projections/mutations/save_todo.mutation.graphql',
					caption: 'Same mutation program as owner complete/archive — force-archive is just another event on this arm.',
					code: `# Syntax-only read-model mutation → MutationProgram IR.
mutation SaveTodo {
  upsert_Todos(object: $input.todo)
}`
				},
				{
					file: 'projections/mutations/delete_todo.mutation.graphql',
					caption: 'Purge arm (if ever elevated) still uses the same delete program.',
					code: `# Syntax-only read-model mutation → MutationProgram IR.
mutation DeleteTodo {
  delete_Todos_by_pk(todo_id: $input.todo_id)
}`
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
					code: `const session = $derived(data.session as SessionLike | null | undefined);
const user = $derived(session?.user);
const engineRole = $derived(
  engineRoleFromGroups(user?.groups)
);`
				},
				{
					file: 'lib/roles.ts',
					caption: 'UI + SSR map IdP groups to admin | user before GraphQL.',
					code: `export function engineRoleFromGroups(
  groups: string[] | undefined,
): 'admin' | 'user' {
  if (!groups?.length) return 'user';
  if (groups.includes('admin') || groups.includes('admins')) {
    return 'admin';
  }
  return 'user';
}

export function isAdminEngineRole(
  role: string | null | undefined,
): boolean {
  return role === 'admin';
}`
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
					file: 'routes/+layout.svelte',
					code: `const pageData = createPageDataSessionSource(initialData);
const client = provideDistributed({
  session: pageData.session,
  browser,
  hydration: initialData.distributed,
  authority: initialData.distributedAuthority,
});`
				},
				{
					file: 'service.rs · oidc_bearer_config',
					caption: 'OIDC claim map → allowlisted engine roles on the session.',
					code: `oidc.claim_map.engine_roles = vec![
  "user".into(),
  "admin".into(),
];
oidc.claim_map.role_claims = vec![
  "groups".into(),
  "roles".into(),
  "realm_access.roles".into(),
  "urn:zitadel:iam:org:project:roles".into(),
];
// Empty identity allowed for e2e-ui-public (anonymous).
oidc.require_auth = false;
IdentityConfig::oidc_bearer(oidc)`
				}
			]
		},
		{
			id: 'handlers',
			label: '3 · Edge map',
			lede: 'OidcBearer validates JWT and injects session variables. Handlers then call ctx.user_id() — repository pattern sits on top of that principal.',
			principle: 'Set-only identity; surface privilege for execution.',
			samples: [
				{
					file: 'src/graphql/identity/resolve.rs',
					code: `IdentityMode::OidcBearer => {
  let oidc = config.oidc.as_ref().ok_or(AuthError::Unauthorized)?;
  match extract_bearer(headers)? {
    None => {
      if oidc.require_auth {
        Err(AuthError::Unauthorized)
      } else {
        Ok(ResolvedIdentity::unverified(Session::new()))
      }
    }
    Some(token) => oidc_identity(&token, oidc),
  }
}`
				},
				{
					file: 'engine · principal_may_open_application',
					code: `fn principal_may_open_application(
  asserted: &[String],
  eligible: &[String],
) -> bool {
  if asserted.is_empty() {
    return eligible.iter().any(|role| role == "anonymous");
  }
  asserted.iter().any(|role| {
    eligible
      .binary_search_by(|c| c.as_str().cmp(role.as_str()))
      .is_ok()
  })
}`
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
					file: 'ui/src/auth.ts · group claims',
					code: `const DEFAULT_GROUP_CLAIMS = [
  'groups',
  'roles',
  'urn:zitadel:iam:org:project:roles',
  'urn:zitadel:iam:org:projects:roles',
];
// Auth.js jwt/session callbacks store access + refresh
// and flatten Zitadel project role keys into session.user.groups`
				}
			]
		},
		{
			id: 'events',
			label: '5 · Directory',
			lede: 'People still appear as AuthUsers via Zitadel ingest/scrape domain events — joins for chat author and blob owner, not a second display-name source.',
			principle: 'Know which side of the fence you are on.',
			samples: [
				{
					file: 'readmodels/models/auth_users.rs · AuthUsers',
					caption: 'Imported IdP directory row. PK is OIDC sub / session x-user-id. Filled by ingest, never by commands.',
					code: `#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ReadModel)]
#[table("auth_users")]
pub struct AuthUsers {
  #[id("user_id")]
  pub user_id: String,
  pub email: String,
  pub display_name: String,
  /// human | machine
  pub user_kind: String,
  /// pending | approved | rejected
  pub approval_status: String,
  /// active | deactivated
  pub status: String,
  pub updated_at: String,
}`
				},
				{
					file: 'handlers/events/project_auth_user.rs',
					code: `pub const EVENTS: &[&str] = &[
  "zitadel.user.human.created.v1",
  "zitadel.user.human.updated.v1",
  "zitadel.user.human.deactivated.v1",
  "zitadel.user.human.reactivated.v1",
  "zitadel.user.machine.created.v1",
];

pub async fn handle<R, L, S>(
  ctx: &Context<'_, AuthDeps<R, L, S>>,
) -> Result<Value, HandlerError> {
  let payload: ZitadelUserPayload = decode_payload(ctx.message())?;
  let name = ctx.message().name();
  let row = if name.contains("deactivated")
    || name.contains("reactivated")
  {
    map_zitadel_user_status(name, &payload)
  } else {
    map_zitadel_user_upsert(name, &payload)
  };
  let store = ctx.read_model_store();
  let mut plan = ReadModelWritePlanBuilder::new();
  plan.upsert(&row).map_err(read_model_error)?;
  plan.commit(store).await.map_err(read_model_error)?;
  Ok(json!({
    "event": name,
    "user_id": row.user_id,
    "status": row.status,
    "display_name": row.display_name,
  }))
}`
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
					file: 'routes/public/+page.svelte · request body',
					code: `{
  "query": "{ chat_messages(limit: 10, offset: 0) { message_id body room_id created_at } }",
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
					file: 'readmodels/models/chat_messages.rs · ChatMessages',
					caption: 'Same lobby model as signed-in chat — surface privilege, not a second shape.',
					code: `#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ReadModel)]
#[readmodel(primary_key = ["message_id"])]
pub struct ChatMessages {
  #[readmodel(id)]
  pub message_id: String,
  pub room_id: String,
  pub author_id: String,
  pub body: String,
  pub created_at: String,
  #[readmodel(belongs_to = "AuthUsers", foreign_key = "author_id")]
  pub author: Option<AuthUsers>,
}`
				},
				{
					file: 'readmodels · anonymous grants',
					caption: 'Only models granted to anonymous appear on this surface.',
					code: `// ChatMessages
.grant("anonymous", read().all_columns())

// AuthUsers (author display joins)
.grant("anonymous", read().all_columns())

// Todos / BlobGames: no anonymous grant
// → absent from the public client schema`
				},
				{
					file: 'service.rs · public surface registration',
					code: `.client_application_surface(
  "e2e-ui-public",
  ["anonymous"],
)`
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
					file: 'generated/public/commands.ts',
					code: `export const COMMAND_ARTIFACTS = [] as const;

export const COMMANDS = {} as const;

export type GeneratedCommands =
  Readonly<Record<never, never>>;`
				},
				{
					file: 'generated/public/sveltekit.ts',
					code: `export function provideDistributed(
  options: Omit<
    CreateDistributedSvelteKitOptions<GeneratedCommands>,
    'createCommands'
  >,
): DistributedSvelteKitClient<GeneratedCommands> {
  return provideDistributedSvelteKitClient(
    createDistributedSvelteKit<GeneratedCommands>({
      ...options,
    }),
  );
}`
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
					file: 'src/graphql/engine/protocol.rs',
					code: `fn principal_may_open_application(
  asserted: &[String],
  eligible: &[String],
) -> bool {
  // Unauthenticated principals may open surfaces that list \`anonymous\`.
  if asserted.is_empty() {
    return eligible.iter().any(|role| role == "anonymous");
  }
  asserted.iter().any(|role| {
    eligible
      .binary_search_by(|c| c.as_str().cmp(role.as_str()))
      .is_ok()
  })
}`
				},
				{
					file: 'src/graphql/identity/resolve.rs · empty Bearer',
					code: `None => {
  if oidc.require_auth {
    Err(AuthError::Unauthorized)
  } else {
    Ok(ResolvedIdentity::unverified(Session::new()))
  }
}`
				}
			]
		},
		{
			id: 'domain',
			label: '4 · Chat model',
			lede: 'Reads still hit the same chat_messages query model that authenticated clients use — only the surface privilege differs.',
			principle: 'Register once, ship everywhere.',
			samples: [
				{
					file: 'readmodels/models/chat_messages.rs',
					code: `impl ChatMessages {
  pub fn permissions() -> ModelPermissions<Self> {
    ModelPermissions::new()
      .grant("user", read().all_columns())
      .grant("admin", read().all_columns())
      .grant("anonymous", read().all_columns())
  }
}`
				}
			]
		},
		{
			id: 'events',
			label: '5 · Events',
			lede: 'Messages still arrive from chat domain events + projection. Public clients only observe what anonymous RLS allows — they cannot post.',
			principle: 'Commands change the world; tables are for reading.',
			samples: [
				{
					file: 'projections/chat.rs',
					code: `projection! {
  pub const CHAT_MESSAGES: ProjectionDescriptor<DirectCandidate> = {
    name: "project_chat_messages",
    version: 1,
    epoch: "e2e-ui-chat-v2",
    model: ChatMessages,
    on {
      events: [ChatMessagePostedDomainEvent],
      mutation: save_chat_message,
      input: { message: body },
    },
  };
}
// ChatMessagePostedDomainEvent → upsert chat_messages
// Public surface has no chat.post command inventory`
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
