import type { DemoWalkthrough } from './types';

/**
 * Tab order is browser-first teaching order for every demo:
 * 1. Query / live subscription (+ ReadModel shape + read RBAC)
 * 2. Commands (optimistic cache vs Atomic / pure reduce) (+ write RBAC)
 * 3. Service module (e2e-service: MODULE_ID, routes(), compose into Service)
 * 4. Command handlers (principal → repo → aggregate → commit)
 * 5. Domain model (plain Rust + macros; blob pure core)
 * 6. Domain events + projections
 *
 * Samples should be real, pasteable shapes from the fixture — not comment-only stubs.
 * Query tabs should include the read model definition (#[derive(ReadModel)] struct), not only
 * permissions snippets or GraphQL selection.
 *
 * Consistency teaching (same mutation IR; apply site differs):
 * - Eventual (placement + command): event handler applies IR async → client
 *   auto-optimism previews until obligations complete (no response row).
 * - Atomic (Direct placement + Atomic command): command handler applies IR
 *   same-tx → wait and return the row; confirm before await settles.
 * - Known-record pure (blob move): client runs domain pure (WASM) for paint;
 *   Atomic seal remains server authority.
 */

export const todosWalkthrough: DemoWalkthrough = {
	id: 'todos',
	href: '/todos',
	title: 'Todos',
	kicker: 'Browser → command → domain → projection',
	summary:
		'Start on the page: one @load query feeds the replica. Commands are Eventual: auto-optimism paints a safe preview from input + domain transition; the event handler applies the same mutation IR later, so the client cannot wait for a response row — only obligations.',
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
			lede: 'Writes go through generated commands. Todos are Eventual: the client applies a safe optimistic preview into the replica cache that feeds the UI, then confirms when the projection obligation completes.',
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
					file: 'modules/todo.rs · todos_create',
					caption: 'Write RBAC + session guard; emit set from domain transition; owner claim is auto-optimism.',
					code: `.command_transition::<
  domain_commands::Create,
  TodoCreateInput,
  Eventual<TodoCreatePayload>,
>(todo_create::COMMAND)
.field_name("todos_create")
.roles(["user", "admin"])
.input_defaults(command_input_defaults! {
  input: TodoCreateInput;
  default input.todo_id = uuid_v7();
})
.guarded(causal_has_user, todo_create::handle)`
				},
				{
					file: 'modules/todo.rs · todos_force_archive',
					caption: 'Elevated mutation: admin role on surface + causal_is_admin guard; not on the user client tree.',
					code: `.command_transition::<
  domain_commands::ForceArchive,
  TodoForceArchiveInput,
  Eventual<TodoForceArchivePayload>,
>(todo_force_archive::COMMAND)
.field_name("todos_force_archive")
.roles(["admin"])
.guarded(causal_is_admin, todo_force_archive::handle)`
				}
			]
		},
		{
			id: 'service',
			label: '3 · Service module',
			lede: 'e2e-service is the application crate. The todo module is one bounded-context slice: MODULE_ID, Routes::for_aggregate, command mounts, and the modeled projector. Domain rules stay in todo-domain; GraphQL surfaces and runners live elsewhere.',
			principle: 'Bounded contexts show up as modules on one Service.',
			samples: [
				{
					file: 'modules/todo.rs · MODULE_ID + routes()',
					caption: 'One module = Todo aggregate inventory + projector attach point.',
					code: `pub const MODULE_ID: &str = "todo";

pub fn routes<R, L, S>(
  repo: R,
  locks: L,
  read_models: S,
  todo_projector: SurfaceProjector,
) -> TodoRoutes<R, L, S> {
  Routes::for_aggregate::<R, L, Todo, S>(repo, locks, read_models)
    .command_transition::</* Create */>(todo_create::COMMAND)
    // … rename, complete, reopen, archive, force_archive, purge …
    .modeled_projector(todo_projector)
    .handle(handlers::events::project_todos::handle)
}`
				},
				{
					file: 'modules/compose.rs · build_service',
					caption: 'compose lists modules; host/runner starts the binary.',
					code: `let todos = todo::routes(repo.clone(), locks.clone(), read_models.clone(), projections.todo);
let chat = chat::routes(/* … */);
let blob = blob::routes(/* … */);

Service::new()
  .named("e2e-ui")
  .without_http_command_routes()
  .routes(todos)
  .routes(chat)
  .routes(blob)`
				}
			]
		},
		{
			id: 'handlers',
			label: '4 · Handlers',
			lede: 'Command handlers use the repository pattern: bind principal (after the mount guard), get or create the aggregate, call a domain method, commit. Todos choose eventual consistency (Eventual + projector).',
			principle: 'Commands change the world; tables are for reading.',
			samples: [
				{
					file: 'handlers/commands/todo_create.rs',
					caption: 'principal → create → domain → publish_events → eventual',
					code: `pub async fn handle(
  ctx: &CausalCommandContext<'_, Todo>,
  input: TodoCreateInput,
) -> Result<PreparedCommand<Eventual<TodoCreatePayload>>, HandlerError> {
  // Session admission already ran on .guarded(causal_has_user, …).
  let owner = principal(ctx)?;
  let repo = ctx.repo();
  let mut todo = repo.create();
  todo.create(&input.todo_id, &owner, &input.title)
    .map_err(rejected)?;
  let state = TodoState::from(&*todo);
  repo.publish_events().commit(todo)?.eventual(TodoCreatePayload {
    todo_id: state.todo_id,
    owner_id: state.owner_id,
    title: state.title,
    status: state.status,
  })
}`
				},
				{
					file: 'handlers/commands/todo_complete.rs',
					code: `let owner = principal(ctx)?;
let mut todo = repo
  .get(&input.todo_id)
  .await?
  .ok_or_else(|| HandlerError::NotFound(input.todo_id.clone()))?;
todo.complete(&owner).map_err(rejected)?;
let state = TodoState::from(&*todo);
repo.publish_events().commit(todo)?.eventual(TodoStatusPayload {
  todo_id: state.todo_id,
  status: state.status,
})`
				}
			]
		},
		{
			id: 'domain',
			label: '5 · Domain',
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
			label: '6 · Events',
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
	kicker: 'Browser → live query → Eventual post',
	summary:
		'Start with the document: @load seeds HTML and @live continues the same query over WebSocket. Posts are optimistic Eventual commands into the shared replica. Guests read via e2e-ui-public.',
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
			lede: 'Post is a generated command for signed-in surfaces only. The client replica cache applies a modeled optimistic message; Eventual confirmation follows the projector.',
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
					file: 'modules/chat.rs · chat_messages_post',
					caption: 'Write RBAC + session guard. Emit set from domain_commands::Post. Public client has zero commands.',
					code: `.command_transition::<
  domain_commands::Post,
  ChatPostInput,
  Eventual<ChatPostPayload>,
>(chat_post::COMMAND)
.field_name("chat_messages_post")
.roles(["user", "admin"])
.guarded(causal_has_user, chat_post::handle)`
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
			id: 'service',
			label: '3 · Service module',
			lede: 'The chat module in e2e-service mounts lobby commands and also hosts identity ingress (Zitadel Action + scrape) plus chat/auth projectors. One module file is the review surface for “how chat joins the Service.”',
			principle: 'Bounded contexts show up as modules on one Service.',
			samples: [
				{
					file: 'modules/chat.rs · routes()',
					caption: 'ChatMessage commands + Zitadel extension commands + projectors.',
					code: `pub const MODULE_ID: &str = "chat";

pub fn routes<R, L, S>(...) -> ChatRoutes<R, L, S> {
  Routes::for_aggregate::<R, L, ChatMessage, S>(repo, locks, read_models)
    .command_transition::</* Post */>(chat_post::COMMAND)
    .guarded(causal_has_user, chat_post::handle)
    // Zitadel Action ingress + on-demand scrape (non-GraphQL).
    .command(zitadel::COMMAND)
    .guarded(zitadel::guard, zitadel::handle)
    .modeled_projector(chat_projector)
    .handle(project_chat_messages::handle)
    .events(project_auth_user::EVENTS)
    .guarded(project_auth_user::guard, project_auth_user::handle)
}`
				},
				{
					file: 'modules/compose.rs · MODULE_IDS',
					caption: 'Inventory is explicit — todo, chat, blob, identity.',
					code: `pub const MODULE_IDS: &[&str] = &[
  todo::MODULE_ID,
  chat::MODULE_ID,
  blob::MODULE_ID,
  "identity",
];`
				}
			]
		},
		{
			id: 'handlers',
			label: '4 · Handlers',
			lede: 'Handler creates the chat aggregate through the repository, applies the domain post, commits Eventual (projector path). Author is the session principal after the mount guard.',
			principle: 'Trust the signed-in person, not the request body.',
			samples: [
				{
					file: 'handlers/commands/chat_post.rs',
					code: `pub async fn handle(
  ctx: &CausalCommandContext<'_, ChatMessage>,
  input: ChatPostInput,
) -> Result<PreparedCommand<Eventual<ChatPostPayload>>, HandlerError> {
  let author = principal(ctx)?;
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
  repo.publish_events().commit(msg)?.eventual(ChatPostPayload {
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
			label: '5 · Domain',
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
			label: '6 · Events',
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
	kicker: 'Browser → pure optimism → Atomic seal',
	summary:
		'One @load query owns the board. Moves are Atomic with thin input. Known-row pure reduce runs blob-domain board rules in WASM so the cache can paint immediately; the handler still runs the same pure, stages the row, and the sealed response remains authority.',
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
			lede: 'Moves are Atomic: send only game_id + direction. The service declares a pure reduce over the known BlobGames row so the client can paint the next board without inventing a second ruleset. Session admission is a mount guard; the sealed Atomic row remains authority.',
			principle: 'One pure for board rules — server and client share the domain core.',
			samples: [
				{
					file: 'routes/blob/[[gameId]]/+page.svelte',
					caption: 'Thin input; ensure WASM is warm for pure-reduce optimism.',
					code: `onMount(() => {
  void ensureBlobWasm();
  /* … */
});

await commands.blob.move({ game_id, direction });
// pure may patch map_json/score/… immediately from known row;
// Atomic seal confirms or corrects`
				},
				{
					file: 'modules/blob.rs · blob.move',
					caption: 'Atomic + pure reduce + session guard. No hand-written TS board twin.',
					code: `.command_transition::<
  domain_commands::MoveDir,
  BlobMoveInput,
  Atomic<BlobGames>,
>(blob_move::COMMAND)
.field_name("blob_games_move")
.roles(["user", "admin"])
.preview_reduce_known_record(
  CommandProjectionPureReduce::new(
    "blob.simulate_move",
    "blob/simulate-move",
    "simulateMove",
    "BlobGames",
  )
  .key_input("game_id", ["game_id"])
  .arg_input("direction", ["direction"])
  .assign(["map_json", "score", "player_dead",
           "current_level_completed", "status"]),
)
.guarded(causal_has_user, blob_move::handle)`
				},
				{
					file: 'ui/src/lib/blob/simulate-move.ts',
					caption: 'Thin WASM host — rules live in blob_domain::core.',
					code: `export function simulateMove(record, args) {
  if (!api) return null; // fail closed until ensureBlobWasm()
  const json = api.blobSimulateMove(
    record.map_json, Number(record.score), args.direction
  );
  return json ? Object.freeze(JSON.parse(json)) : null;
}`
				}
			]
		},
		{
			id: 'service',
			label: '3 · Service module',
			lede: 'The blob module in e2e-service is only Atomic command mounts for BlobGame — no projector loop for the board (direct placement seals in the handler). compose() still lists MODULE_ID next to todo and chat.',
			principle: 'Bounded contexts show up as modules on one Service.',
			samples: [
				{
					file: 'modules/blob.rs · MODULE_ID + routes()',
					caption: 'start / move / start_level — all Atomic, all guarded.',
					code: `pub const MODULE_ID: &str = "blob";

pub fn routes<R, L, S>(...) -> BlobRoutes<R, L, S> {
  Routes::for_aggregate::<R, L, BlobGame, S>(repo, locks, read_models)
    .command_transition::</* StartWithMap */>(blob_start::COMMAND)
    .guarded(causal_has_user, blob_start::handle)
    .command_transition::</* MoveDir */>(blob_move::COMMAND)
    .preview_reduce_known_record(/* blob.simulate_move */)
    .guarded(causal_has_user, blob_move::handle)
    .command_transition::</* StartLevel */>(blob_start_level::COMMAND)
    .guarded(causal_has_user, blob_start_level::handle)
}`
				},
				{
					file: 'modules/compose.rs · blob routes',
					caption: 'Same Service composition as todos/chat.',
					code: `let blob = blob::routes(repo, locks, read_models, projections.blob);

Service::new()
  .named("e2e-ui")
  .without_http_command_routes()
  .routes(todos)
  .routes(chat)
  .routes(blob)`
				}
			]
		},
		{
			id: 'handlers',
			label: '4 · Handlers',
			lede: 'Get aggregate, domain move (which calls the same pure as WASM), stage the mutation-derived row, commit Atomic — one transaction for aggregate, ledger, and query row. Input parse stays here; session admission already ran on the guard.',
			principle: 'Commands change the world; tables are for reading.',
			samples: [
				{
					file: 'handlers/commands/blob_move.rs',
					code: `pub async fn handle(
  ctx: &CausalCommandContext<'_, BlobGame>,
  input: BlobMoveInput,
) -> Result<PreparedCommand<Atomic<BlobGames>>, HandlerError> {
  let owner = principal(ctx)?;
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
    .atomic()
}`
				}
			]
		},
		{
			id: 'domain',
			label: '5 · Domain',
			lede: 'One blob-domain crate: core is pure board rules (WASM-eligible); models is the sourced aggregate (feature domain). move_dir calls core::simulate_move — the same function the client WASM exports.',
			principle: 'Start with the domain, not the database.',
			samples: [
				{
					file: 'blob-domain/src/core · simulate_move',
					caption: 'Pure map + score + direction → next board. No Entity, no ownership.',
					code: `pub fn simulate_move(
  map: &[Vec<u8>],
  score: i64,
  direction: Direction,
) -> Result<MovePreview, SimulateError> {
  // step player, mark visited, hole/suicide/score, level complete…
  Ok(MovePreview { map, score, player_dead, level_complete })
}`
				},
				{
					file: 'blob-domain · BlobGame::move_dir',
					caption: 'Aggregate wraps pure + ownership + domain event.',
					code: `pub fn move_dir(&mut self, owner_id: &str, direction: Direction)
  -> Result<(), BlobError>
{
  self.ensure_owner(owner_id)?;
  if self.player_dead { return Err(BlobError::PlayerDead); }
  let preview = simulate_move(&self.map, self.score, direction)
    .map_err(map_simulate_err)?;
  self.record_moved(
    preview.score,
    preview.player_dead,
    preview.level_complete,
    preview.map,
    direction.as_str().to_string(),
  )?;
  Ok(())
}`
				},
				{
					file: 'blob-domain · features',
					caption: 'Server uses domain; client pure uses wasm.',
					code: `// Cargo.toml
// default = ["domain"]  → aggregate + levels + distributed
// wasm                 → blobSimulateMove export (make wasm)`
				}
			]
		},
		{
			id: 'events',
			label: '6 · Events',
			lede: 'Domain events still exist for history. For blob, the same save_blob_game mutation program runs direct in the command handler (same commit as the event) — so the response can carry the row. Eventual placement would run that IR in an event handler instead, with no response channel to the waiting client.',
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
					file: 'service · projection binding',
					code: `let blob_binding = ProjectionBinding::materialize_direct(
  BLOB_GAMES.direct(),
  source(),
  owner("project_blob"),
  /* … */,
  vec![projection_output::<BlobGames>()],
  /* … */,
)?;
// Handler stages the row via readmodel(row).commit()?.atomic()`
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
		'Start with the elevated query on e2e-ui-admin. Force-archive is an Eventual command on that surface only — still optimistic cache on the admin replica, still repo → aggregate → commit on the server.',
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
					file: 'modules/todo.rs · todos_force_archive',
					caption: 'Write RBAC: admin only + causal_is_admin guard; absent from user client inventory.',
					code: `.command_transition::<
  domain_commands::ForceArchive,
  TodoForceArchiveInput,
  Eventual<TodoForceArchivePayload>,
>(todo_force_archive::COMMAND)
.field_name("todos_force_archive")
.roles(["admin"])
.guarded(causal_is_admin, todo_force_archive::handle)`
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
			id: 'service',
			label: '3 · Service module',
			lede: 'Admin is not a second service binary — it is a second GraphQL surface (e2e-ui-admin) over the same e2e-service. Force-archive is mounted once in modules/todo.rs; only the admin client inventory includes it.',
			principle: 'Roles and surfaces are real; modules stay shared.',
			samples: [
				{
					file: 'modules/todo.rs · force_archive mount',
					caption: 'Same module that serves /todos — elevated field on the same Routes inventory.',
					code: `.command_transition::<
  domain_commands::ForceArchive,
  /* … */,
>(todo_force_archive::COMMAND)
.field_name("todos_force_archive")
.roles(["admin"])
.guarded(causal_is_admin, todo_force_archive::handle)`
				},
				{
					file: 'modules/graphql.rs · dual surfaces',
					caption: 'Service crate opens user + admin + public application surfaces.',
					code: `.client_application_surface_with_schema_roles(
  "e2e-ui",
  ["admin", "user"],
  ["user"],
)
.client_application_surface("e2e-ui-admin", ["admin"])
.client_application_surface("e2e-ui-public", ["anonymous"])`
				}
			]
		},
		{
			id: 'handlers',
			label: '4 · Handlers',
			lede: 'Handler still uses repo.get → domain force_archive → Eventual commit. Authorization is role + surface + mount guard, not a special HTTP path.',
			principle: 'Trust the signed-in person, not the request body.',
			samples: [
				{
					file: 'handlers/commands/todo_force_archive.rs',
					code: `pub async fn handle(
  ctx: &CausalCommandContext<'_, Todo>,
  input: TodoForceArchiveInput,
) -> Result<PreparedCommand<Eventual<TodoForceArchivePayload>>, HandlerError> {
  let admin = principal(ctx)?;
  let repo = ctx.repo();
  let mut todo = repo
    .get(&input.todo_id)
    .await?
    .ok_or_else(|| HandlerError::NotFound(input.todo_id.clone()))?;

  todo.force_archive().map_err(rejected)?;
  let state = TodoState::from(&*todo);
  repo.publish_events()
    .commit(todo)?
    .eventual(TodoForceArchivePayload {
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
			label: '5 · Domain',
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
			label: '6 · Events',
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
			id: 'claims',
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
			id: 'service',
			label: '5 · Service module',
			lede: 'Identity is not a domain aggregate in e2e-service — it is service-crate wiring: OIDC/dev headers, claim maps, and the chat module’s Zitadel ingress that fills AuthUsers. MODULE_IDS still lists "identity" next to todo/chat/blob.',
			principle: 'Transport identity, compose modules, keep domains pure.',
			samples: [
				{
					file: 'modules/compose.rs · MODULE_IDS',
					caption: 'Identity is an inventory slot; command mounts live on chat/todo/blob.',
					code: `pub const MODULE_IDS: &[&str] = &[
  todo::MODULE_ID,
  chat::MODULE_ID,
  blob::MODULE_ID,
  "identity",
];`
				},
				{
					file: 'modules/chat.rs · Zitadel ingress',
					caption: 'Service-crate extension commands (not GraphQL user mutations).',
					code: `.command(handlers::ingestors::zitadel::COMMAND)
.guarded(zitadel::guard, zitadel::handle)
.command(handlers::ingestors::zitadel_scrape::COMMAND)
.guarded(zitadel_scrape::guard, zitadel_scrape::handle)
.events(project_auth_user::EVENTS)
.guarded(project_auth_user::guard, project_auth_user::handle)`
				}
			]
		},
		{
			id: 'events',
			label: '6 · Directory',
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

export const allWalkthroughs: DemoWalkthrough[] = [
	todosWalkthrough,
	chatWalkthrough,
	blobWalkthrough,
	adminWalkthrough,
	sessionWalkthrough
];

export function walkthroughById(id: string): DemoWalkthrough | undefined {
	return allWalkthroughs.find((d) => d.id === id);
}
