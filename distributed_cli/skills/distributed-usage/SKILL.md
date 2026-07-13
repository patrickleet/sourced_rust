---
name: distributed-usage
description: Build Distributed CQRS/event-sourced Rust services where you mostly write models and handlers while the framework and dctl generate persistence, transports, manifests, and deploy wiring. Use model-first TDD to specify plain aggregate behavior with fast unit tests before implementing models and thin handlers. Use when designing, testing, writing, or modifying a Distributed service or domain model.
---

# Using the Distributed framework

**The point of Distributed: you mostly just write models and handlers.**

**The model-first advantage: specify and exhaustively test plain domain models
first, then write thin handlers around proven behavior.**

Everything else — service wiring, transports, persistence, manifests, schema,
CI/GitOps — is deterministic structure the framework, macros, and `dctl`
generate. Your authored surface is deliberately small: aggregate models
(`#[sourced]` event methods), command/event handler bodies, and read-model
shapes. If you find yourself hand-writing service plumbing, routing, broker
topology, or deploy YAML, stop — the framework or CLI almost certainly
generates it, and hand-rolled copies drift.

**Always reach for the highest-level API first.** The macros and one-call
conveniences are the recommended surface, not sugar:

- `#[sourced]` — not `#[digest]` + `aggregate!()` (its lower-level building
  blocks, still supported but only for granular control you actually need)
- `#[derive(Snapshot)]` / `#[derive(ReadModel)]` — not hand-written snapshot
  payloads or table plumbing
- `routes!` + `Service::routes(..)` — not manual handler registration
- `service.with_bus(bus).run(opts)` — not direct `listen`/`subscribe`/
  `send`/`publish` wiring (drop to the facade only for finer control)

Dropping a level means re-implementing conventions the macro layer keeps
correct for you (event enums, replay, subscription plans, outbox publication
on commit). Do it deliberately, not by default.

Distributed is a CQRS + event-sourcing framework for Rust. Domain state lives in
plain structs; `#[sourced]` command methods record replayable `EventRecord`s;
read models serve queries; published messages are created deliberately through
the outbox. Infrastructure (storage, bus, locks) is pluggable behind async
traits — production swaps are one constructor line, never a handler change.

## Workflow

1. Scaffold a service instead of hand-rolling layout:
   `dctl scaffold <name> --model <agg> --command <agg.action> --event <fact.happened> --store postgres --transport http --bus nats --gitops`
   (from an event-storming board: aggregates → `--model`, commands →
   `--command`, events/policies → `--event`, query views → `--read-models`).
2. Write failing, colocated unit tests against the aggregate command API you
   want. Exercise the plain model directly, without a handler or infrastructure.
3. Implement the model fields, event methods, validation, and guards until the
   contract tests pass. Refactor while they stay green.
4. Implement thin handlers around the proven model behavior. Keep the generated
   structure around them.
5. Start in-memory (`InMemoryRepository`, `InMemoryBus`); swap constructors for
   Postgres/a broker when deploying. Handlers do not change.

## Model-first TDD

Treat an aggregate's public command methods as the domain API. Define and prove
that API before implementing handlers or services:

1. Write the behavior you want as a failing test.
2. Instantiate the plain model and call its command methods directly. Do not use
   a repository, bus, handler `Context`, async runtime, database, or mocks.
3. Assert the complete observable contract: returned result and resulting
   state/snapshot; decode the generated event enum for its name and payload, and
   assert the record version, sequence, and event count separately.
4. Cover every valid transition, validation failure/domain rejection, guard or
   no-op, repeated call, invariant, and boundary case. An explicit rejection or
   intentional guard/no-op must not mutate state or add a pending event.
5. Implement the smallest model behavior that makes the test pass, then
   refactor. Repeat until the model modules have 100% coverage.
6. Only then add handlers. A handler should decode input, load or create the
   aggregate, invoke an already-proven command method, then commit. Add an
   outbox message only when a fact must be published outside the aggregate.

Use the generated event enum and `TryFrom<&EventRecord>` to assert business
facts; do not couple domain tests to encoded payload bytes. `Entity::events()`
is the aggregate's complete in-memory history, while `Entity::new_events()` is
the set added since it was loaded or committed.

A `#[event(..., when = condition)]` command returns `Ok(())` when its condition
is false: this is a successful no-op, not a domain error. If the API should
reject instead, test for `Err`, unchanged state, and no new event, then validate
in a public command method before calling a private recorded event applier. An
error returned from a fallible recorded event body does not roll back the event
that was already recorded, so do not use that as the rejection boundary.

Run `cargo llvm-cov --lib --summary-only` in the bounded-context crate to check
model coverage. Coverage confirms execution; the result, state, invariant, and
event assertions above define the actual contract.

## Aggregates

An aggregate is a plain struct embedding `distributed::Entity`. `#[sourced]` on
its impl block turns `#[event("...")]` methods into recorded events and
generates the typed event enum + `Aggregate` impl.

```rust
#[derive(Default, Snapshot)]
struct Todo {
    entity: Entity,
    task: String,
    completed: bool,
}

#[sourced(entity, aggregate_type = "todo")]
impl Todo {
    #[event("initialized")]
    fn initialize(&mut self, id: String, task: String) {
        self.entity.set_id(&id);
        self.task = task;
    }

    #[event("completed", when = !self.completed)]
    fn complete(&mut self) {
        self.completed = true;
    }
}
```

Rules that prevent real bugs:

- **Always set an explicit `aggregate_type`** — the default is the Rust type
  name, which silently changes the durable stream identity if the type is
  renamed.
- **Event methods become fallible.** `#[event]`/`#[digest]` methods return
  `SourcedResult` even without a declared return type — call them with `?`.
- Event names are lowercase past-tense facts (`"initialized"`, `"completed"`).
  Use `when = <expr>` for intentional idempotent/no-op transitions so they
  record nothing; use explicit validation when the API promises a rejection.
- Evolving an event's payload? Do not edit stored data — add an upcaster:
  `#[sourced(entity, upcasters(("initialized", 1 => 2, V1 => V2, upcast_fn)))]`
  and mark new-format methods `#[event("initialized", version = 2)]`.

## Command handlers

One module per handler, exporting `COMMAND` (or `EVENT`/`EVENTS`), a `guard`,
and an async `handle`:

```rust
pub const COMMAND: &str = "todo.initialize";

pub fn guard(ctx: &Context<Repo>) -> bool {
    ctx.has_fields(&["id", "task"])
}

pub async fn handle(ctx: &Context<'_, Repo>) -> Result<Value, HandlerError> {
    let input = ctx.input::<CreateTodo>()?;
    let mut todo = Todo::default();
    todo.initialize(input.id.clone(), input.task)?;
    let message = OutboxMessage::domain_event("todo.initialized", &todo)?;
    ctx.repo().outbox(message).commit(&mut todo).await?;
    Ok(json!({ "id": input.id }))
}
```

Register with `routes!` and serve — the same handlers work over direct
dispatch, HTTP (`microsvc::serve`, feature `http`), gRPC (feature `grpc`), and
the bus:

```rust
let routes = distributed::routes!(
    Routes::new().with_repo(repo.queued().aggregate::<Todo>()),
    command handlers::todo_create,
    command handlers::todo_complete,
);
let service = Service::new().named("todo-api").routes(routes);
service.with_bus(bus).run(RunOptions::idempotent()).await?;
```

## Publication is explicit (outbox)

An `EventRecord` is write-side replay history, **not** automatically a domain
event other services see. To publish a fact, create an `OutboxMessage` and
commit it with the aggregate — one transaction, durable delivery:

```rust
let message = OutboxMessage::domain_event("todo.initialized", &todo)?;
repo.outbox(message).commit(&mut todo).await?;
```

With a bus attached (`service.with_bus(bus)`), commit publishes immediately;
without one, rows stay pending for an `OutboxDispatcher` worker. Use
`OutboxMessage::encode_for_entity(id, name, &payload, &entity)` for custom
payloads and automatic correlation/causation metadata propagation.

## Read models

`#[derive(ReadModel)]` declares a query-optimized relational projection:

```rust
#[derive(Clone, Debug, Serialize, Deserialize, ReadModel)]
#[table("todo_views")]
pub struct TodoView {
    #[id]
    pub id: String,
    pub task: String,
    #[jsonb]
    pub metadata: serde_json::Value,
}
```

Two update modes — pick deliberately:

- **Atomic** (same service, response must reflect the write):
  `repo.read_models(plan).commit(&mut agg).await?` commits rows and events in
  one transaction.
- **Eventual** (cross-service): a projector subscribes to published messages
  and commits through `ctx.read_model_store().workspace()`, which marks the
  message processed in the same transaction for idempotency.

## Choosing infrastructure

| Concern | Dev/test | Production |
|---|---|---|
| Storage | `InMemoryRepository` | `PostgresRepository::connect_and_migrate(url)` (feature `postgres`); `SqliteRepository` for single-node |
| Bus | `InMemoryBus` | `NatsBus`, `RabbitBus`, `KafkaBus`, `PostgresBus`/`SqliteBus` (same DB, no broker), `KnativeBus` |
| Locking | `InMemoryLockManager` (via `.queued()`) | `PostgresLockManager` via `.queued_with(locks)` for cross-process serialization |

Gotchas:

- Application crates depend on `distributed` only — the macros are re-exported;
  never add `distributed_macros` directly.
- Keep the shared bounded-context crate feature-light; enable `postgres`/
  `http`/`nats`/... only in executable service crates.
- `Service::named(..)` is the durable consumer group — same name for every
  replica of one deployment. `namespace(..)` scopes broker topology per
  app/environment. Names are validated: portable IDs only (`A-Za-z0-9_-`,
  `.` also allowed in namespaces, max 128 bytes).
- `microsvc` does **not** authenticate. Deploy behind a trusted proxy that
  strips client-supplied identity headers and injects authenticated claims
  (`Session` is an opaque map; `x-user-id` / `x-role` are convenience keys only).
- `connect_and_migrate` applies migrations; plain `connect` does not create
  tables.

## Multi-crate single-service layout (and later microservices)

When a service grows past one aggregate, copy the **workshop-service** fixture
layout under `tests/workshop-service/` (also documented in
[[specs/workshop-domain]] and that folder’s README):

```text
crates/
  <bc>-domain/     # aggregates + events only
  <bc2>-domain/
  readmodels/      # all BC projections in one package
  service/         # composable route bundles (handlers)
  runner-*/        # thin bins: choose store + transport + bind
```

**Start as one process:** register every route bundle on a single `Service`
(`build_full_service`). Domain unit tests stay pure; behavioral suite speaks
HTTP/GraphQL only.

**Split into microservices without redefining the domain:** run the same
handler modules in different processes (`build_catalog_service` vs
`build_orders_service`). Share bus + event store + read-model tables. Add a
gateway only for unified GraphQL and command routing by name prefix.

The same suite case IDs must pass for monolith and multi-service topologies
(`cargo test -p workshop-suite --test behavioral` and `--test multi_service`).

## Manifest entrypoint

Every service should export `distributed_manifest()` registering its read
models — `dctl describe` and `dctl schema` compile the crate and call it:

```rust
pub fn distributed_manifest() -> distributed::DistributedProjectManifest {
    distributed::DistributedProjectManifest::new("todos").read_model::<TodoView>()
}
```

Keep it updated when adding read models or tables; see the
`distributed-schema` skill for rendering schema artifacts from it.

## References

- Framework guide: the `distributed` crate README (https://crates.io/crates/distributed)
- Read models: `docs/read-models.md`; transports: `docs/transports.md`;
  repositories: `docs/repositories.md` in the Distributed repo
- CI/GitOps scaffolding: the `distributed-ci` skill
- Schema and manifest tooling: the `distributed-schema` skill
