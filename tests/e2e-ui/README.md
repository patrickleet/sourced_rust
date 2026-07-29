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

pub const TODO_READS: ProjectionDescriptor<EventualOnly> = projection! {
    name: "project_todos";
    version: 1;
    epoch: "e2e-ui-todos-v2";
    partition: unit;

    on [
        "todo.created",
        "todo.renamed",
        "todo.completed",
        "todo.reopened",
        "todo.archived"
    ] version 1 (state: TodoState) {
        upsert Todos from state as todo;
    }

    on "todo.purged" version 1 (deleted: TodoDeletionIdentity) {
        delete Todos { key { todo_id: envelope.aggregate_id } };
    }
};
```

The command registration links the command to its possible domain event and
optionally previews only values known before dispatch:

```rust
typed_command::<TodoCompleteInput, Causal<TodoStatusPayload>>("todo.complete")
    .emits(events![TodoCompletedDomainEvent])
    .preview(state_preview! {
        TodoCompletedDomainEvent => TodoState {
            todo_id: input.todo_id,
            status: "completed",
            ..unknown
        }
    })
```

The compiler specializes `TODO_READS` into safe client operations. Known
fields become an optimistic patch; unknown fields use narrow recovery or
revalidation. Actual emitted occurrences—not the declaration—mint exact
obligations for the active projector binding. A no-op command therefore emits
zero occurrences, mints zero obligations, and completes as `Succeeded`.

Handlers use the fluent unit of work:

```rust
let state = TodoState::from(&*todo);
ctx.publish_events()
    .commit(todo)?
    .causal(TodoStatusPayload {
        todo_id: state.todo_id,
        status: state.status,
    })
```

Blob uses the same projection model through the direct terminal:

```rust
let view = BlobGames::from(&game.state());
ctx.project(BLOB_GAMES).commit(game)?.projected(view)
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
The projection syntax supports upsert, patch, delete, link, unlink, model or
relationship invalidation, and client revalidation fallback.

## Portable projections and stateful read-model work

`projection!` is the portable semantic program shared by eventual server
consumers, eligible same-transaction execution, and client optimism. It can
fan one event out to multiple tables and relationship operations.

The existing fluent read-model workspace remains the escape hatch for
data-dependent, stateful work: load current rows/relationships, apply arbitrary
multi-table ORM plans, and commit atomically. That is not merely an ACK with a
subscription; its declared output scopes still produce causal obligations.
What does not execute in the browser becomes a scoped invalidation and
revalidation.

Query-only relationships can be composed at the deployment boundary without a
crate cycle or a second projection ORM. This fixture adds
`BlobGames.owner`, `ChatMessages.author`, and the reverse
`AuthUserView.blob_games`/`chat_messages` relationships to the canonical model
schemas. Projection storage identity ignores only that query metadata and
continues to pin every physical column, type, key, and table identity.

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
| `crates/todo-domain/src/projection.rs` | State lifecycle, partial preview helper, and explicit purge deletion. |
| `crates/chat-domain/src/projection.rs` | Insert-shaped causal projection. |
| `crates/blob-domain/src/projection.rs` | Direct projection plus compile-fail eligibility guards. |
| `crates/service/src/service.rs` | Deployment catalog, active bindings, routes, grants, and typed commands. |
| `crates/readmodels/src/lib.rs` | Provider view and deployment-composed cross-domain relationships. |
| `ui/src/routes/*/+page.graphql` | Co-located SSR/live reads. |
| `ui/src/routes/*/+page.svelte` | `*.use()` and ordinary typed command calls. |
| `ui/src/lib/generated/` | Generator-owned user/admin clients; do not hand-edit. |

Todo and Chat mount catalog-pinned local causal executors with
`consume_projection`. Blob has a catalog-pinned direct owner and no
asynchronous event route or second writer. The Zitadel provider ingestor is an
intentional integration adapter, not a portable aggregate projection.

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
