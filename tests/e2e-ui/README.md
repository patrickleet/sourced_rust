# e2e-ui template

A copyable Distributed service and SvelteKit UI demonstrating one modeled
projection from aggregate transition to server read model, generated GraphQL
client, optimistic replica update, and causal confirmation.

```bash
cd tests/e2e-ui
make up
set -a && source e2e-ui.env && set +a
make run
```

The UI is at `http://localhost:5180`; GraphQL is at
`http://127.0.0.1:8791/graphql`. Demo users are `alice`, `bob`, and `admin`
with password `Password1!`.

## The developer experience

The page code stays ordinary:

```ts
const todos = Todos.use();
const chat = ChatMessages.use();
const games = BlobGames.use();
const commands = useCommands();

await commands.todo.complete({ todo_id });
```

The Rust side declares what happened and how it changes query models:

```rust
#[event("todo.completed", version = 1, domain)]
fn record_completed(&mut self) {
    self.status = TodoStatus::Completed;
}

// 1) Mutation (event-free, GraphQL-looking syntax-only — not a public GQL field)
// src/mutations/save_todo.mutation.graphql:
//   mutation SaveTodo { upsert_Todos(object: $input.todo) }
pub fn save_todo() -> Mutation<()> {
    mutation_file!("src/mutations/save_todo.mutation.graphql")
}
pub fn delete_todo() -> Mutation<()> {
    mutation_file!("src/mutations/delete_todo.mutation.graphql")
}

// 2) Portable handlers: on <events> apply <mutation> (event-first)
projection! {
    pub const TODOS: ProjectionDescriptor<EventualOnly> = {
        name: "project_todos",
        version: 1,
        epoch: "e2e-ui-todos-v2",
        model: Todos,
        on_event {
            TodoCreatedDomainEvent,
            TodoCompletedDomainEvent, /* … */
        }
        apply save_todo as "todo",
        on_deleted TodoPurgedDomainEvent => apply delete_todo as "todo_id",
    };
}
```

Command registration declares the domain events this command may emit and the
**known mutation input** used for client cache application (not a separate
hand-built cache path):

```rust
typed_command::<TodoCompleteInput, Causal<TodoStatusPayload>>("todo.complete")
    .emits(events![TodoCompletedDomainEvent])
    .applies(state_preview! {
        TodoCompletedDomainEvent => TodoState {
            todo_id: input.todo_id,
            status: "completed",
            ..unknown
        }
    })
```

The compiler specializes `TODOS` into safe client operations: apply the same
mutation IR to the cache with known fields only; unknown fields use narrow
recovery or revalidation. Actual emitted occurrences—not the declaration—mint
exact obligations for the active projector binding. A no-op command therefore
emits zero occurrences, mints zero obligations, and completes as `Succeeded`.

Handlers use the fluent unit of work (eventual — projector applies the mutation):

```rust
let repo = ctx.repo();
let mut todo = repo
    .get(&input.todo_id)
    .await?
    .ok_or_else(|| HandlerError::NotFound(input.todo_id.clone()))?;
todo.complete(&owner).map_err(rejected)?;

let state = TodoState::from(&*todo);
repo.publish_events()
    .commit(todo)?
    .causal(TodoStatusPayload {
        todo_id: state.todo_id,
        status: state.status,
    })
```

Blob uses a **handler-owned projected** commit: materialize the row from the
same mutation used for event bindings, stage it, and seal `Projected`:

```rust
let repo = ctx.repo();
let mut game = repo
    .get(&input.game_id)
    .await?
    .ok_or_else(|| HandlerError::NotFound(input.game_id.clone()))?;
game.move_dir(&owner, direction).map_err(rejected)?;

let row = save_blob_game()
    .from_state(&BlobGameState::from(&*game))?;
repo.readmodel(row)
    .publish_events()
    .commit(game)?
    .projected()
```

`Projected<BlobGames>` means aggregate history, command ledger, read-model row,
and response evidence commit atomically. Its deliberately narrow eligibility
is one complete row upsert; patches, deletes, multi-row programs, and stateful
relationship work remain eventual.

## Vocabulary

- An **aggregate event** is write-side history used to replay the aggregate.
- A **domain event** is an outward fact other components may react to.
- `#[event(..., domain)]` is shorthand for the common case where the aggregate
  event name is suitable outwardly and its post-transition `DomainState` is
  the useful body.
- A **DomainState** is a versioned public post-transition DTO. It may omit
  secrets, internal counters, and replay-only details.
- A **snapshot** is a private aggregate rehydration optimization. It is not
  automatically an integration contract.
- A **read model** is query-shaped storage such as `Todos`, `ChatMessages`, or
  `BlobGames`.

When state transfer is unsuitable, use an explicit sparse domain-event DTO.
`mutation!` supports upsert, patch, delete, link, unlink, model or relationship
invalidation; projections bind events to mutation inputs.

## Portable mutations and modeled projectors

`mutation!` is the public, event-independent authoring path for read-model
writes. Portable/modeled event handlers bind domain occurrences to mutation
programs; the same IR lowers on the server and for role-safe client cache
optimism. Multi-model atomicity is expressed as multi-op mutation programs, not
a public projector ORM workspace.

Application commands declare `.emits` plus `.applies(...)` known mutation-input
mapping for client cache application. Eventual commands do not stage rows in
the handler; Blob stages the mutation-derived row with
`readmodel(row).commit()?.projected()`.

Query relationships are declared once on the referencing read model, without a
second projection ORM. This fixture adds `Todos.owner`, `BlobGames.owner`, and
`ChatMessages.author`, each referencing `AuthUsers`. The foreign-key side is
the single relationship declaration; `AuthUsers` does not need reverse
collections for every model that references it.

Read RBAC is attached to those same query models through
`Todos::permissions()`, `BlobGames::permissions()`, and their peers. The
runtime GraphQL engine and generated application surfaces reuse those
declarations; `service.rs` does not recreate the grants.

## Outcomes

| Outcome | Guarantee |
|---|---|
| `Succeeded<T>` | Command transaction succeeded; no projection wait is promised. |
| `Causal<T>` | Actual emitted occurrences created durable obligations for the exact active causal projector bindings. Zero actual occurrences complete immediately as succeeded. |
| `Projected<T>` | The eligible canonical read-model row committed in the command transaction and is returned as evidence. |

`Accepted` is reserved for genuine fire-and-forget transport acceptance, not
the normal GraphQL command result.

## Code index

| Path | Purpose |
|---|---|
| `crates/*-domain/` | Aggregates, replay events, and outward domain-state contracts. |
| `crates/readmodels/` | Query shapes, relationships, and model-owned read RBAC. |
| `crates/projections/` | Todo, Chat, and Blob event-to-read-model programs. |
| `crates/service/src/handlers/events/` | Explicit projector handlers that apply modeled programs. |
| `crates/service/src/handlers/commands/` | Repository → get/create → domain operation → fluent commit. |
| `crates/service/src/service.rs` | Deployment catalog, placement, routes, and typed commands. |
| `ui/src/routes/*/+page.graphql` | Co-located SSR/live reads. |
| `ui/src/routes/*/+page.svelte` | `*.use()` and ordinary typed command calls. |
| `ui/src/lib/generated/` | Generator-owned user/admin clients; do not hand-edit. |

Todo and Chat mount catalog-pinned local causal executors through explicit
`modeled_projector(...).handle(...)` event handlers. Those handlers apply the
shared plan without repeating its mapping. Blob has a catalog-pinned direct
owner and no asynchronous event route or second writer. The Zitadel provider
ingestor remains an integration adapter, while its provider-event-to-`AuthUsers`
mapping also lives in `e2e-projections` and runs from an explicit event handler.

## Generation and tests

```bash
make gen-client
make check-client
make test
make test-live
make test-browser
```

`make test-live` needs the local Postgres/Zitadel stack. Browser tests also need
the UI/API processes and a checked-in Playwright runtime. The always-on offline
path uses SQLite plus `DevHeaders`; production-shaped identity uses
`OidcBearer`.

After changing a model, command, projection, grant, or `+page.graphql`, run the
supported generator and commit its deterministic output. Never repair
generated files by hand.

## Security and deployment

Owner/author values come from `ctx.user_id()`, never free-form client input.
GraphQL row grants enforce `owner_id = claim("x-user-id")`; the admin mutation
tree is a separate generated application surface. Disable GraphiQL outside
local development.

See [PROJECTION_ROLLOUT.md](PROJECTION_ROLLOUT.md) for compatibility probes,
remote activation, drain, rebuild/import, rollback, and obligation-minting
controls.
