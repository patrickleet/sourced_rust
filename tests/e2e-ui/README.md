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

// 2) Projections: on { events, mutation, input } (event-first)
projection! {
    pub const TODOS: ProjectionDescriptor<EventualOnly> = {
        name: "project_todos",
        version: 1,
        epoch: "e2e-ui-todos-v2",
        model: Todos,
        on {
            events: [
                TodoCreatedDomainEvent,
                TodoCompletedDomainEvent, /* … */
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
}
```

Command registration declares the domain events this command may emit and the
**known mutation input** used for client cache application (not a separate
hand-built cache path):

```rust
typed_command::<TodoCompleteInput, Eventual<TodoStatusPayload>>("todo.complete")
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
same mutation used for event bindings, stage it, and seal `Atomic`:

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

`Atomic<BlobGames>` means aggregate history, command ledger, read-model row,
and response evidence commit atomically. Its deliberately narrow eligibility
is one complete row upsert; patches, deletes, multi-row programs, and stateful
relationship work remain eventual.

### Ship contract: same IR, two response proofs (agents)

There is **one** portable mutation program (e.g. `save_blob_game` /
`save_todo`). Placement chooses *where* it runs. The **command response**
differs on purpose — do **not** collapse them into “always send a causal delta.”

| Contract | Apply site | Mutation response (ship) | Client seal |
|---|---|---|---|
| **`Eventual<T>`** + Eventual | Event handler after commit | Payload + **projection-delta** + `expects` | `.applies` preview; retire on obligations |
| **`Atomic<M>`** + Direct | Command handler, same tx | **Typed row `M`** + direct **`records[]`**. No eventual modeled metadata, empty `expects` | Optional `.applies`; **`confirmDirectProjection(row, records)`** before `await` settles |

Handler for Atomic — this *is* returning atomic read-model updates:

```rust
let row = save_blob_game().from_state(&BlobGameState::from(&*game))?;
repo.readmodel(row).publish_events().commit(game)?.projected()
// GraphQL returns BlobGames; extensions.records from same-tx evidence.
```

Server enforces the split: same-transaction commands **do not** persist eventual
modeled projection metadata (`routes.rs`). The typed row + records *are* the
atomic proof — not a re-encoded causal delta.

Do **not**:

- reimplement domain rules in the UI (“board-sim”) for Atomic commands;
- require a causal projection-delta on Atomic responses (client bug / wrong API);
- treat Direct as “no client program” — Direct may export preview IR for `.applies`
  (`is_preview_eligible`) without becoming Eventual;
- conflate `is_causally_eligible` (Eventual-only obligations) with preview eligibility.

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
mapping for client cache application. Both Eventual and Direct surfaces may
export those previews from the portable program. Eventual commands do not stage
rows in the handler (the event handler applies the mutation later). Blob stages
the mutation-derived row with `readmodel(row).commit()?.projected()` so the
GraphQL response carries the authoritative row — possible only because apply
happens in the command handler, not in a later event handler.

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
| `Eventual<T>` | Actual emitted occurrences created durable obligations for the exact active causal projector bindings. Zero actual occurrences complete immediately as succeeded. |
| `Atomic<T>` | Eligible canonical read-model row committed in the command transaction **and returned on the response** (handler-owned apply). Client normalizes that row before the call settles. |

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
shared plan without repeating its mapping — that is why the waiting client only
has `.applies` previews until obligations complete. Blob has a catalog-pinned
direct owner and no asynchronous event route or second writer: the command
handler applies the same mutation IR and returns the row. The Zitadel provider
ingestor remains an integration adapter, while its provider-event-to-`AuthUsers`
mapping also lives in `e2e-projections` and runs from an explicit event handler.

## Application composition (framework direction)

e2e-ui is the Full-process reference. Long-term wiring should follow
[docs/application-composition.md](../../docs/application-composition.md):

- **Logical:** command defs + projection mounts + surfaces (product graph)
- **Process role:** Full | CommandWriter | EventualProjector | QueryApi
- **Runtime:** store + locks + bus + workers + GraphQL (dialect / host)

Atomic commands stay collocated with their write path; Eventual projectors
may run in another process on the same packages.

## Generation and tests

```bash
make gen-client
make check-client
make test
make test-browser
```

### Optimism regression gates

Client optimism is proven in two layers:

1. **Offline artifacts** (`ui/tests/optimism-artifacts.test.mjs`, run via `make ui-test` /
   UI `npm test`): every demo write command must export non-empty
   `projection.preview.operations`; Atomic also needs `directProjection`.
2. **Browser paint-before-wire** (the Chat, Todo, and Blob product journeys plus
   `e2e/helpers/optimism.ts`): hold the GraphQL mutation response longer than the
   assert deadline and require the UI to update first. The same journey then
   proves authoritative convergence and continuity.

```bash
# offline
cd ui && npm test -- tests/optimism-artifacts.test.mjs

# browser (stack up: make up && make run)
npx playwright test e2e/chat.user.spec.ts e2e/todos.user.spec.ts \
  e2e/blob.user.spec.ts --project=chromium-user
```

Browser tests need the local Postgres/Zitadel stack, UI/API processes, and a
checked-in Playwright runtime. The always-on offline path uses SQLite plus
`DevHeaders`; the root GraphQL identity and live provider suites own detailed
`OidcBearer` isolation/spoof/401 coverage.

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
