# Distributed

Distributed is a CQRS and event-sourcing framework for Rust applications that want simple domain models, replayable aggregate history, durable publication, and pluggable infrastructure.

It keeps your domain model as a plain struct (Plain Old Rust Struct, or PORS), inspired by POCO/POJO, while giving you append-only aggregate event records, replay, snapshots, read models, an outbox, a multi-transport service bus, and a small async command-handler framework.

The core idea is explicit boundaries: aggregate event records are the write-side source of truth, read models serve queries, and published domain or integration messages are created deliberately through the outbox.

It is built with stateless vertical and horizontal scaling in cloud-native environments in mind. You can start with a single in-memory service and split it later into partitioned services backed by Postgres and a real broker — without rewriting the domain model.

> **The framework is async-only.** Aggregates, repositories, handlers, the commit
> path, and the service bus are all `async`. There is no synchronous repository or
> bus API. Persistence adapters (Postgres, SQLite) and transports (NATS, RabbitMQ,
> Kafka, Knative) implement the async traits directly with no blocking shims.

## At a Glance

| Capability | What it gives you |
|---|---|
| Plain Rust aggregates | Domain state stays in ordinary structs with explicit command methods. |
| Event-sourced persistence | Append-only `EventRecord`s, replay, optimistic commit, and pluggable async repositories. |
| Typed macros | `#[sourced]`, `#[digest]`, and `aggregate!()` remove boilerplate while keeping replay explicit. |
| Snapshots | `#[derive(Snapshot)]` and a snapshot cache speed up hydration for long streams. |
| Outbox | Durable publication records committed atomically with aggregates. |
| Read models | Query-optimized relational projections, committed atomically or updated eventually. |
| Service bus facade | `send`/`listen` (point-to-point) and `publish`/`subscribe` (fan-out) over a swappable transport. |
| Transports | In-memory, Postgres, NATS JetStream, RabbitMQ, Kafka, and Knative/CloudEvents — one constructor line apart. |
| Microservice framework | Convention-based async handlers exposed over HTTP, gRPC, the bus, or direct dispatch. |
| Pluggable infrastructure | Async traits for storage, messaging, read models, snapshots, outbox publishing, and locking. |

## Quick Start

Four steps: write your models, write a command handler, serve it, then swap in
production persistence and transports without touching any of the above.

### 1. Write your models

A domain model is a plain Rust struct with an embedded `Entity`. `#[sourced]` turns
its command methods into recorded, replayable events; `#[derive(Snapshot)]` adds a
hydration cache for long streams.

```rust
use serde::Deserialize;
use distributed::{sourced, Entity, Snapshot};

#[derive(Default, Snapshot)]
struct Todo {
    entity: Entity,
    user_id: String,
    task: String,
    completed: bool,
}

#[sourced(entity, aggregate_type = "todo")]
impl Todo {
    #[event("initialized")]
    fn initialize(&mut self, id: String, user_id: String, task: String) {
        self.entity.set_id(&id);
        self.user_id = user_id;
        self.task = task;
    }

    #[event("completed", when = !self.completed)]
    fn complete(&mut self) {
        self.completed = true;
    }
}

// The command input your handler decodes
#[derive(Deserialize)]
struct CreateTodo {
    id: String,
    user_id: String,
    task: String,
}

// #[sourced] generates: TodoEvent enum, TryFrom<&EventRecord>, impl Aggregate
// #[derive(Snapshot)] generates: TodoSnapshot, fn snapshot(), impl Snapshottable
```

### 2. Write a command handler

Each handler is a module exporting a `COMMAND` name, a `guard`, and an **async**
`handle`. It loads/creates the aggregate, runs a command, and commits the resulting
events — optionally alongside a durable outbox message in the same transaction.

```rust
// handlers/todo_create.rs
use serde_json::{json, Value};
use distributed::microsvc::{Context, HandlerError};
use distributed::OutboxMessage;

use super::Repo; // an AggregateRepository<_, Todo> alias

pub const COMMAND: &str = "todo.initialize";

pub fn guard(ctx: &Context<Repo>) -> bool {
    ctx.has_fields(&["id", "user_id", "task"])
}

pub async fn handle(ctx: &Context<'_, Repo>) -> Result<Value, HandlerError> {
    let input = ctx.input::<CreateTodo>()?;

    let mut todo = Todo::default();
    todo.initialize(input.id.clone(), input.user_id, input.task)?;

    // Record a fact for other services. The outbox row commits atomically with
    // the aggregate's events. Once a bus is attached (step 3) this `commit`
    // publishes the row immediately; with no bus it stays pending for a worker.
    let message = OutboxMessage::domain_event("todo.initialized", &todo)?;
    ctx.repo().outbox(message).commit(&mut todo).await?;

    Ok(json!({ "id": input.id }))
}
```

### 3. Serve it

Build the service fluently from `Service::new()`, register handlers with
`register_handlers!`, then expose the exact same service over direct dispatch,
HTTP, gRPC, or the bus. Handlers are written once and are transport-agnostic.

```rust
use std::sync::Arc;
use distributed::microsvc::{self, Service, Session};
use distributed::bus::{InMemoryBus, RunOptions};
use distributed::{AggregateBuilder, HashMapRepository, Queueable};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let service = distributed::register_handlers!(
        Service::new().with_repo(
            HashMapRepository::new()
                .queued()
                .aggregate::<Todo>()
        ),
        command handlers::todo_create,
        command handlers::todo_complete,
    );

    // Attach a bus and run. `with_bus` closes the loop from step 2: that
    // `outbox(..).commit(..)` now publishes on commit, and `run` consumes the
    // registered commands (and events). Same handlers, one line of wiring.
    service
        .with_bus(InMemoryBus::new())
        .run(RunOptions::idempotent())
        .await?;

    // Alternatives that share the same handlers:
    //   service.dispatch("todo.initialize", json!({ "id": "todo-1", .. }), Session::new()).await?; // in-process
    //   microsvc::serve(Arc::new(service), "0.0.0.0:3000").await?;     // HTTP (feature = "http")
    //   microsvc::serve_grpc(Arc::new(service), "[::1]:50051").await?; // gRPC (feature = "grpc")

    Ok(())
}
```

### 4. Swap persistence and transports

Everything above is in-memory. Moving to production is a **constructor change**, not
a handler change — every infrastructure concern is an async trait with an in-memory
default you replace with a durable adapter.

```rust
// Persistence: HashMapRepository → durable SQL (features "postgres" / "sqlite")
let repo = distributed::PostgresRepository::connect_and_migrate(database_url).await?;
let service = distributed::register_handlers!(
    Service::new().with_repo(repo.queued().aggregate::<Todo>()),
    command handlers::todo_create,
    command handlers::todo_complete,
);

// Transport: InMemoryBus → a real broker. The handlers and the
// `with_bus(..).run(..)` wiring are unchanged; only this constructor line differs.
//   let bus = NatsBus::connect("nats://localhost:4222", "todos", "app").await?;
//   let bus = PostgresBus::new(pool, "todos");
//   let bus = RabbitBus::connect("amqp://localhost:5672/%2f", "todos", "app").await?;
//   let bus = KafkaBus::connect("localhost:9092", "todos", "app").await?;
service.with_bus(bus).run(RunOptions::idempotent()).await?;
```

| Concern | In-memory default | Swap in for production |
|---|---|---|
| Storage | `HashMapRepository` | `PostgresRepository`, `SqliteRepository` |
| Messaging | `InMemoryBus` | `NatsBus`, `PostgresBus`, `RabbitBus`, `KafkaBus`, `KnativeBus` |
| Locking | `InMemoryAsyncLockManager` | `PostgresLockManager`, `SqliteLockManager` (durable leases), any `AsyncLockManager` (Redis, …) |

The rest of this README is the reference guide for each of these pieces.

## Example Conventions

Examples use production-style error propagation. Event methods generated by `#[sourced]` and `#[digest]`, repository calls, and outbox constructors are fallible, so snippets that call them assume a surrounding `async` function that returns a `Result` and use `?` / `.await?`.

Complete runnable examples live under [`tests/`](tests/). Short snippets focus on the API surface and may omit surrounding imports or application-specific types when those are not the point of the example.

## Project Inspiration

Distributed is inspired by the original [sourced](https://github.com/mateodelnorte/sourced) Node.js project by Matt Walters and his accompanying [servicebus](https://github.com/mateodelnorte/servicebus) library for distributed messaging. Patrick Lee Scott, a contributor and maintainer of the original JavaScript/TypeScript versions, brought these concepts to Rust and refactored them for the Rust ecosystem. The bus facade (`send`/`listen` + `publish`/`subscribe`, with per-transport `*Bus` types) mirrors the `servicebus` / `rabbitbus` / `kafkabus` / `knativebus` family.

## Design Goals

- Keep domain objects simple and explicit (Plain Old Rust Structs).
- Make aggregate event records the source of truth for model state.
- Make replay predictable and safe.
- Keep storage and messaging pluggable and testable behind async traits.
- Make the transport a wiring choice, not a handler change.
- Add optional queue-based locking for serialized workflows.

## Feature Flags

The in-memory repository and the service bus facade are part of the core crate and
always available. Optional features pull in transports, persistence adapters, and
network servers.

| Feature | Default | Adds |
|---|---:|---|
| `emitter` | Yes | In-process event emission and `#[enqueue]`. |
| `http` | No | Axum HTTP transport for `microsvc` + the Knative/CloudEvents ingress router. |
| `grpc` | No | Tonic gRPC transport for `microsvc`. |
| `postgres` | No | `PostgresRepository` and the Postgres outbox/transport (`PostgresBus`). |
| `sqlite` | No | `SqliteRepository` async SQL adapter for local persistence and conformance. |
| `nats` | No | `NatsBus` (NATS JetStream source/publisher). |
| `rabbitmq` | No | `RabbitBus` (RabbitMQ source/publisher). |
| `kafka` | No | `KafkaBus` (Kafka source/publisher). |

> The `InMemoryBus` and `PostgresBus` need no broker feature beyond `postgres` for
> Postgres; the in-memory bus is always available for dev and tests.

## Core Concepts

- **Entity**: Holds the event history. You embed it in your domain structs.
- **EventRecord**: An immutable aggregate event record with name, payload, sequence, timestamp, and optional metadata. It is replayable model history, not automatically a published domain event.
- **Aggregate**: A struct that embeds an `Entity` and replays `EventRecord`s. `aggregate_type()` provides the durable stream-identity component for persistence.
- **Repository / AggregateRepository**: Persists and loads aggregates by event history. The event store is optimized for append and replay; `get`/`commit` are async.
- **HashMapRepository**: In-memory repository for tests and examples. Implements every async trait (repository, read-model, snapshot, outbox).
- **SqliteRepository / PostgresRepository**: Durable async SQL adapters (optional features).
- **QueuedRepository**: Wraps any repository and adds async per-entity queue locking.
- **EventUpcaster**: A pure, stateless transformation that converts event payloads from one version to another at read time.
- **Snapshottable**: Opt-in trait for aggregates that produce state snapshot payload DTOs. Use `#[derive(Snapshot)]` to auto-generate the payload struct and trait impl.
- **OutboxMessage**: A durable publication work item for a domain event, integration event, command, or generic transport message. Supports optional `destination` for point-to-point routing and metadata propagation.
- **OutboxDispatcher / OutboxWorker**: Drain durable outbox rows and publish them to a transport, sharing one claim → publish → complete path.
- **ReadModel**: Query-optimized relational projection state for UI/API reads. Read models may be updated atomically with a command or eventually from published messages.
- **Bus / BusConsumer**: The service bus facade — `send`/`publish` (produce) and `listen`/`subscribe` (consume), implemented by a per-transport `*Bus` type.
- **microsvc::Service**: Convention-based async command/event handler framework with pluggable transports (HTTP, gRPC, bus, direct dispatch).

## Terminology And CQRS Boundaries

Event sourcing is the model-level persistence strategy: aggregates record replayable `EventRecord`s when command methods such as `#[event]` (within `#[sourced]`) or `#[digest]` methods succeed. Those records are the write-side history used to hydrate the aggregate.

CQRS is the architectural split between write-side aggregates and query-side read models. Repositories load aggregate event streams by ID for command handling; production business queries should read from `ReadModel` projections shaped for that query.

Published messages are a separate boundary. An aggregate event record is not automatically a domain event. When other services, projections, or transports need a fact or command, create an `OutboxMessage` and commit it with the aggregate. The outbox payload can represent a domain event, integration event, command, or any other transport message.

The existing names and serialized fields such as `EventRecord::event_name` remain part of the compatibility contract. Terminology cleanup should clarify usage without renaming stored event records unless a migration path is explicitly designed.

## Pluggable by Default

Every infrastructure concern in `distributed` follows the same pattern: an **async trait** defines the contract, an **in-memory implementation** ships out of the box for testing and development, and you swap in your own for production.

| Concern | Async trait(s) | In-memory default | Swap in for production |
|---|---|---|---|
| Storage | `GetStream` + `TransactionalCommit` | `HashMapRepository` | `PostgresRepository`, `SqliteRepository`, … |
| Messaging | `Bus` + `BusConsumer` | `InMemoryBus` | `NatsBus`, `PostgresBus`, `RabbitBus`, `KafkaBus`, `KnativeBus` |
| Read model rows | `ReadModelWritePlanStore` + `RelationalReadModelQueryStore` | `InMemoryReadModelStore` | Postgres, SQLite |
| Snapshot store | `SnapshotStore` | `InMemorySnapshotStore` | Postgres, SQLite, … |
| Outbox publishing | `AsyncMessagePublisher` / `OutboxPublisher` | `LogPublisher` | Any transport publisher |
| Locking | `AsyncLock` + `AsyncLockManager` | `InMemoryAsyncLockManager` | `PostgresLockManager`, `SqliteLockManager` (durable leases), Redis, … |

All in-memory defaults are `Clone` and `Send + Sync`, so they work in single-task tests and multi-task servers alike. When you're ready for production, implement the trait for your infrastructure and plug it in — handler code does not change.

## The `#[sourced]` Macro

The `#[sourced]` attribute macro is the recommended way to define event-sourced aggregates. Place it on an impl block and annotate command methods with lowercase, past-tense aggregate event names such as `#[event("initialized")]`. It replaces both `#[digest]` and `aggregate!()`, and auto-generates a typed event enum plus the `Aggregate` impl.

Event methods are rewritten to return `SourcedResult`, even when the source method omits an explicit return type. Call them with `?` in application code so serialization and event-recording failures are propagated.

### Basic Usage

```rust
use distributed::{sourced, Entity};

#[derive(Default)]
struct Todo {
    entity: Entity,
    user_id: String,
    task: String,
    completed: bool,
}

#[sourced(entity)]
impl Todo {
    #[event("initialized")]
    fn initialize(&mut self, id: String, user_id: String, task: String) {
        self.entity.set_id(&id);
        self.user_id = user_id;
        self.task = task;
    }

    #[event("completed", when = !self.completed)]
    fn complete(&mut self) {
        self.completed = true;
    }
}
```

This generates:

```rust
// Typed event enum with named fields from method parameters
#[derive(Debug, Clone, PartialEq)]
pub enum TodoEvent {
    Initialized { id: String, user_id: String, task: String },
    Completed,
}

impl TodoEvent {
    pub fn event_name(&self) -> &'static str { /* ... */ }
}

// Convert stored events to typed enum
impl TryFrom<&EventRecord> for TodoEvent { /* ... */ }

// Full Aggregate trait impl (entity accessors + replay logic)
impl Aggregate for Todo { /* ... */ }
```

### Durable Stream Identity

`Aggregate::aggregate_type()` provides the type component of a persistence stream's identity (the pair `(aggregate_type, aggregate_id)`). The default uses Rust's type name for development convenience, but **production persistence should set an explicit, stable durable name**:

```rust
#[sourced(entity, aggregate_type = "todo")]
impl Todo {
    // events are stored under the durable stream type "todo"
}
```

### Using the Typed Event Enum

The generated enum enables exhaustive matching — if you add or remove an event, the compiler tells you everywhere that needs updating:

```rust
use distributed::EventRecord;

fn print_todo_event(record: &EventRecord) -> Result<(), String> {
    let event = TodoEvent::try_from(record)?;
    match event {
        TodoEvent::Initialized { id, user_id, task } => {
            println!("Todo {} created by {}: {}", id, user_id, task);
        }
        TodoEvent::Completed => println!("Todo completed"),
    }
    Ok(())
}
```

### Custom Enum Name

```rust
#[sourced(entity, events = "TodoCommand")]
impl Todo {
    // generates TodoCommand enum instead of TodoEvent
}
```

### Versioned Events

Create events at a specific version for [upcasting](#event-upcasting--versioning):

```rust
type InitV1 = (String, String);
type InitV2 = (String, String, u8);

fn upcast_init_v1_v2((id, task): InitV1) -> InitV2 {
    (id, task, 0)
}

#[sourced(entity, upcasters(
    ("initialized", 1 => 2, InitV1 => InitV2, upcast_init_v1_v2),
))]
impl TodoV2 {
    #[event("initialized", version = 2)]
    fn initialize(&mut self, id: String, task: String, priority: u8) {
        // creates events at version 2
    }

    #[event("completed", when = !self.completed)]
    fn complete(&mut self) {
        self.completed = true;
    }
}
```

### Custom Entity Field

```rust
#[sourced(my_entity)]
impl MyAggregate {
    #[event("initialized")]
    fn create(&mut self, name: String) {
        // uses self.my_entity
    }
}
```

### With `enqueue` for Choreography

Add `enqueue` to `#[sourced]` to automatically queue events for in-process emission alongside digest. Every `#[event]` method both records to the entity stream and enqueues for emission:

```rust
use distributed::{sourced, Entity};
use distributed::emitter::EntityEmitter;

#[derive(Default)]
struct Order {
    entity: Entity,
    emitter: EntityEmitter,
    status: String,
}

#[sourced(entity, enqueue)]
impl Order {
    #[event("initialized")]
    fn create(&mut self, order_id: String, customer: String) {
        self.entity.set_id(&order_id);
        self.status = "created".into();
    }

    #[event("shipped", when = self.status == "created")]
    fn ship(&mut self) {
        self.status = "shipped".into();
    }
}
```

**Custom emitter field** — when your emitter field isn't named `emitter`:

```rust
#[sourced(entity, enqueue(my_emitter))]
impl Notifier {
    #[event("sent")]
    fn send(&mut self, id: String, message: String) {
        self.entity.set_id(&id);
        self.message = message;
    }
}
```

## The `#[digest]` Macro and `aggregate!()` Macro

The `#[digest]` and `aggregate!()` macros are the lower-level building blocks that `#[sourced]` replaces. They're still fully supported and useful when you want more granular control. Like `#[event]` methods, `#[digest]` methods become fallible and should be called with `?`.

### The `#[digest]` Macro

```rust
// Basic — captures function parameters
#[digest("initialized")]
fn initialize(&mut self, id: String, user_id: String, task: String) {
    self.entity.set_id(&id);
    self.user_id = user_id;
    self.task = task;
}

// Guard conditions — only emit when the condition is true
#[digest("completed", when = !self.completed)]
fn complete(&mut self) {
    self.completed = true;
}

// Versioned events
#[digest("initialized", version = 2)]
fn initialize(&mut self, id: String, task: String, priority: u8) { /* ... */ }

// Custom entity field
#[digest(my_entity, "initialized")]
fn create(&mut self, name: String) { /* uses self.my_entity */ }
```

### The `aggregate!` Macro

Generates the `Aggregate` trait implementation with replay logic:

```rust
aggregate!(Todo, entity, aggregate_type = "todo" {
    "initialized"(id, user_id, task) => initialize,
    "completed"() => complete(),
});
```

With [upcasters](#event-upcasting--versioning) for event schema evolution:

```rust
type InitV1 = (String, String);
type InitV2 = (String, String, u8);

fn upcast_initialized_v1_v2((id, task): InitV1) -> InitV2 {
    (id, task, 0)
}

aggregate!(Todo, entity {
    "initialized"(id, task, priority) => initialize,
    "completed"() => complete(),
} upcasters [
    ("initialized", 1 => 2, InitV1 => InitV2, upcast_initialized_v1_v2),
]);
```

## Event Metadata

Metadata lets you attach cross-cutting context — correlation IDs, causation IDs, user context, trace spans — to events without changing your domain model.

### Setting Metadata on an Entity

Set metadata on the entity before calling command methods. Every event produced by `#[event]` or `#[digest]` automatically inherits it:

```rust
let mut todo = Todo::default();

todo.entity.set_correlation_id("req-abc-123");
todo.entity.set_causation_id("cmd-create-todo");
todo.entity.set_meta("user_id", "u-42");

todo.initialize("todo-1".into(), "user-1".into(), "Ship it".into())?;

assert_eq!(todo.entity.events()[0].correlation_id(), Some("req-abc-123"));
```

Entity metadata is **transient** — it is not serialized with the entity. It is a request-scoped context you set before each command invocation.

### Propagating Metadata to Outbox Messages

Use `encode_for_entity` to create outbox messages that automatically inherit the entity's metadata context:

```rust
let outbox = OutboxMessage::encode_for_entity(
    format!("{}:created", order.entity.id()),
    "order.initialized",
    &payload,
    &order.entity,  // metadata propagates automatically
)?;

repo.outbox(outbox).commit(&mut order).await?;
```

The metadata flows through the full chain:

```text
Entity.set_correlation_id("req-123")
  → #[event] / #[digest] → EventRecord.metadata
  → encode_for_entity → OutboxMessage.metadata
  → OutboxDispatcher → transport Message.metadata
  → subscriber receives the message with correlation_id() == "req-123"
```

Framework-derived metadata (codec, destination, source aggregate) is namespaced under the reserved `x-sourced-` prefix so it cannot be shadowed by user metadata.

### Reading Metadata

```rust
// On EventRecord (event store)
event_record.correlation_id()  // Option<&str>
event_record.causation_id()
event_record.meta("user_id")

// On OutboxMessage
message.correlation_id()
message.meta("trace_id")
```

## In-Process Event Choreography (requires `emitter` feature)

The `emitter` feature (enabled by default) adds in-process event-driven choreography — queue local events during commands and emit them after commit for reactive workflows within a single process.

### With `#[sourced(entity, enqueue)]`

Every `#[event]` method automatically records to the entity stream (for replay) and enqueues for in-process emission:

```rust
use serde::{Deserialize, Serialize};
use distributed::{sourced, Entity};
use distributed::emitter::EntityEmitter;

#[derive(Default, Serialize, Deserialize)]
struct OrderSaga {
    entity: Entity,
    #[serde(skip, default)]
    emitter: EntityEmitter,
    order_id: String,
    status: String,
}

#[sourced(entity, enqueue)]
impl OrderSaga {
    #[event("started")]
    fn start(&mut self, order_id: String) {
        self.entity.set_id(&order_id);
        self.order_id = order_id;
        self.status = "started".into();
    }

    #[event("completed", when = self.status == "started")]
    fn complete_step(&mut self) {
        self.status = "completed".into();
    }
}
```

### Emitting After Commit

Queued events are held until you explicitly emit them after a successful commit:

```rust
let mut saga = OrderSaga::default();
saga.start("order-1".into())?;

// Commit the aggregate...
repo.commit(&mut saga).await?;

// Then emit queued events to registered listeners
saga.emitter.emit_queued();
```

### Registering Listeners

```rust
let shared_state = Arc::new(Mutex::new(Vec::new()));
let state = Arc::clone(&shared_state);

saga.emitter.on("started", move |payload: String| {
    if let Ok(mut events) = state.lock() {
        events.push(payload);
    }
});
```

This pattern is useful for reactive workflows within the same process. For cross-service messaging, use the [Outbox Pattern](#outbox-pattern) and [Service Bus](#service-bus).

## Queued Repository

Per-entity async locking for serialized workflows. `get` acquires the lock, `commit` releases it:

```rust
use distributed::{AggregateBuilder, HashMapRepository, Queueable, RepositoryError};

let repo = HashMapRepository::new().queued().aggregate::<Todo>();

let Some(mut todo) = repo.get("todo-1").await? else {
    return Err(RepositoryError::NotFound { id: "todo-1".into() });
}; // locks this ID
// ... mutate ...
repo.commit(&mut todo).await?; // unlocks

// Or release without changes:
repo.abort(&todo).await?;

// Read without locking:
let _ = repo.peek("todo-1").await?;
```

By default, locking is in-memory (`InMemoryAsyncLockManager`) — process-local, lost
on restart. For **cross-process** serialization, back the queue with a durable
SQLx lease lock (feature `postgres` or `sqlite`). It implements the same
`AsyncLockManager` trait, so it's a drop-in via `queued_with`:

```rust
use distributed::{PostgresLockManager, PostgresRepository};

let repo = PostgresRepository::connect_and_migrate(&database_url).await?;
// The `aggregate_locks` lease table is created by the repository's migrations.
let locks = PostgresLockManager::new(repo.pool().clone());
let todos = repo.queued_with(locks).aggregate::<Todo>();
```

The lease records each held key in the `aggregate_locks` table (`SqliteLockManager`
is the SQLite equivalent). It is a **mutual-exclusion optimization, not a fencing
guarantee** — the event store's `(aggregate_type, aggregate_id, sequence)` primary
key remains the authoritative concurrency boundary. v1 has **no lease renewal**, so
set the lease TTL above your longest critical section. Tune with `with_lease_ttl`,
`with_retry_interval`, and `with_max_wait`; reclaim rows from crashed holders with
`sweep_expired`. Any custom `AsyncLockManager` (e.g. Redis) plugs in the same way.

## Persistent Repositories

The optional `sqlite` and `postgres` features add async, SQL-backed repositories
that implement the same async traits as `HashMapRepository`. They persist aggregate
event streams, relational read-model write plans, processed-message marks,
snapshots, and outbox rows — staging everything through one SQL transaction when
committed via `CommitBatch`.

```rust
// SQLite — local persistence and conformance (requires `sqlite`)
let repo = distributed::SqliteRepository::connect_and_migrate("sqlite::memory:").await?;

// Postgres — the production SQL event-store path (requires `postgres`)
let repo = distributed::PostgresRepository::connect_and_migrate(database_url).await?;
```

`connect_and_migrate` applies the explicit migrations under `migrations/`. Plain
`connect` from an existing pool does **not** create tables implicitly, so
applications can control bootstrap order.

Postgres is the low-ops starter: a single Postgres cluster can back repositories,
read models, the outbox, **and** the durable transport (`PostgresBus`). See
[`docs/repositories.md`](docs/repositories.md) for the full guide.

## Outbox Pattern

Each outbox message is a durable delivery row committed alongside your domain entity. Aggregate event records are write-side replay history; they become domain events, integration events, commands, or transport messages only when application code creates an `OutboxMessage` for that purpose.

```rust
use distributed::OutboxMessage;

let mut todo = Todo::default();
todo.entity.set_correlation_id("req-abc");
todo.initialize("todo-1".into(), "user-1".into(), "Buy milk".into())?;

// Derives id, snapshot payload, and metadata from the aggregate automatically
let message = OutboxMessage::domain_event("todo.initialized", &todo)?;

// Commit both in one repository transaction
repo.outbox(message).commit(&mut todo).await?;
```

For custom payloads or IDs, use `encode_for_entity`:

```rust
let message = OutboxMessage::encode_for_entity(
    format!("{}:init", todo.entity.id()),
    "todo.initialized",
    &custom_payload,
    &todo.entity,
)?;
```

### Publishing the Outbox

How a committed row reaches the bus depends on whether a bus is attached to the
service:

- **Bus attached (`service.with_bus(bus)`)** — `repo.outbox(msg).commit(agg)`
  commits the row, then **immediately** after commit claims it under a short
  lease and publishes it. A crash before the publish, or a publish failure,
  leaves the row claimed under that lease; when the lease expires the polling
  worker takes it.
- **No bus** — the row is committed `pending` and a worker publishes it.

The polling worker is the durable backstop in both cases. It is the same
`OutboxDispatcher` primitive composed with your runtime's timer — run it in the
service process or as a separate worker, against the same outbox store:

```rust
use distributed::{BusPublisher, OutboxDispatcher};
use std::{sync::Arc, time::Duration};

let dispatcher = OutboxDispatcher::new(
    repo.outbox_store(),
    BusPublisher::new(Arc::new(bus)),   // routes commands/events by kind
    "outbox-worker-1",
    Duration::from_secs(30),            // claim lease
    5,                                  // max publish attempts
);

loop {
    dispatcher.dispatch_batch(100).await?;          // claim → publish → complete
    tokio::time::sleep(Duration::from_secs(1)).await;
}
```

A row completes only after `publish()` resolves `Ok`; an unknown or failed publish
leaves it retryable (released until the attempt ceiling, then moved to `Failed`).
Claims use leases, so the immediate path and competing workers never publish the
same row concurrently.

## Service Bus

The service bus is a thin, ergonomic facade over the transport adapters. It exposes
two messaging patterns through two traits:

- **`Bus` (produce)** — `send` a point-to-point command (1:1, competing consumers) or `publish` a fan-out event (1:N).
- **`BusConsumer` (consume)** — `listen` for commands (competing) or `subscribe` to events (fan-out). `listen`/`subscribe` derive the message names from the service's registered handlers, build the transport's source with the right topology, and run it through the shared runner — handler code never changes.

A concrete `*Bus` implements both, so the **application surface is identical across
transports; only the constructor line changes.**

```rust
use std::sync::Arc;
use distributed::bus::{Bus, BusConsumer, InMemoryBus, RunOptions};

// Built once — handlers are transport-agnostic.
let service = Arc::new(build_service());

// Dev/test: in-memory.
let bus = InMemoryBus::new();
bus.send("place.bet", payload).await?;          // point-to-point command (1:1)
bus.publish("seat.reserved", payload).await?;   // fan-out event (1:N)
bus.listen(service.clone(), RunOptions::idempotent()).await?;     // competing
bus.subscribe(service.clone(), RunOptions::idempotent()).await?;  // fan-out

// Production: swap the one constructor line — send/listen/publish/subscribe
// and the handlers are unchanged.
//   let bus = NatsBus::connect("nats://localhost:4222", "orders", "app").await?;
//   let bus = PostgresBus::new(pool, "orders");
//   let bus = RabbitBus::connect("amqp://localhost:5672/%2f", "orders", "app").await?;
//   let bus = KafkaBus::connect("localhost:9092", "orders", "app").await?;
```

This is the low-level facade. For a `microsvc::Service`, the one-call convenience
is `service.with_bus(bus).run(opts)`: it derives the command names to `listen`
and the event names to `subscribe` from the registered handlers, and makes
`repo.outbox(msg).commit(agg)` publish on commit. Drop to `listen` / `subscribe`
/ `send` / `publish` directly when you need finer control.

Point-to-point vs fan-out is consistently a **consumer-group/identity** choice in
each transport's native topology — the same `group` competes, different `group`s
fan out:

| `*Bus` | Feature | `send` / `listen` (competing) | `publish` / `subscribe` (fan-out) |
|---|---|---|---|
| `InMemoryBus` | (always) | named queue, popped once | retained log + per-subscriber cursor |
| `PostgresBus` | `postgres` | `bus_queue`, `FOR UPDATE SKIP LOCKED` | `bus_log` + `bus_offset` per group (Kafka-style) |
| `NatsBus` | `nats` | shared durable `{group}_cmd` on the stream | durable `{group}_evt` per group |
| `RabbitBus` | `rabbitmq` | default exchange → durable queue `{ns}.cmd.{name}` | topic exchange → queue `{ns}.evt.{group}` per group |
| `KafkaBus` | `kafka` | shared consumer group `{ns}.{group}.cmd` | consumer group per service `{ns}.{group}.evt` |
| `KnativeBus` | `http` | POST CloudEvent → `{target}-commands` broker ingress | POST → `{source}-events` broker; consume via generated Triggers |

`KnativeBus` implements only `Bus` (produce → broker-ingress POST). It has no
in-process consume loop: `KnativeBus::manifests(&plan, &subscriptions)` renders the
role-based `Broker` + per-name `Trigger` YAML, and the service mounts
`cloud_events_router` so those Triggers reach `dispatch_message`.

### Idempotency and Failure Policy

`RunOptions::idempotent()` enables idempotent dispatch by default. `RunOptions` also
carries a `FailurePolicy` controlling what happens to a **permanent** handler
failure — `Retry`, `DeadLetter`, `Park`, `LogAndAck`, or `Stop`:

```rust
use distributed::bus::{FailurePolicy, RunOptions};

bus.listen(
    service.clone(),
    RunOptions::idempotent().with_failure_policy(FailurePolicy::Stop),
).await?;
```

Retryable failures (e.g. transient `NotFound`) are nacked for redelivery; the runner
never silently acks a handler error.

See [`docs/async-transports.md`](docs/async-transports.md) for the full transport
layer, the two confirmation thresholds (producer publish vs consumer ack), and the
low-level `AsyncMessageSource` / `AsyncMessagePublisher` / `run_source` boundary the
facade is built on.

## Microservice Framework (`microsvc`)

The `microsvc` module provides a convention-based async command/event handler framework. Register handlers on a `Service<D>`, then expose them over HTTP, gRPC, the bus, or direct dispatch.

### Defining a Service

A `Service<D>` is generic over a dependency type `D` that handlers read via `ctx`. Build one fluently from `Service::new()`: add `.with_repo(repo)` for aggregate command handlers, `.with_read_model_store(store)` for projection handlers (chain both when a handler needs both), and `.with_bus(bus)` to consume from / publish to a transport.

Handlers are registered with a fluent builder. `.command(name)` / `.event(name)` start a registration; `.handle(closure)` adds an unguarded handler and `.guarded(guard, closure)` adds a guarded one. The handler closure receives `&Context<D>` and returns a future:

```rust
use std::sync::Arc;
use distributed::microsvc::{Context, HandlerError, Service, Session};
use distributed::{AggregateBuilder, HashMapRepository, Queueable};
use serde_json::json;

let service = Arc::new(
    Service::new().with_repo(HashMapRepository::new().queued().aggregate::<Counter>())
        .command("counter.initialize")
        .handle(|ctx: &Context<Repo>| {
            let input = ctx.input::<CreateCounter>();
            async move {
                let input = input?;
                let mut counter = Counter::default();
                counter.create(input.id.clone())?;
                ctx.repo().commit(&mut counter).await?;
                Ok(json!({ "id": input.id }))
            }
        })
        .command("counter.increment")
        .handle(|ctx: &Context<Repo>| {
            let input = ctx.input::<IncrementCounter>();
            async move {
                let input = input?;
                let mut counter = ctx.repo().get(&input.id).await?
                    .ok_or_else(|| HandlerError::NotFound(input.id.clone()))?;
                counter.increment(input.amount)?;
                ctx.repo().commit(&mut counter).await?;
                Ok(json!({ "value": counter.value }))
            }
        })
);

// Direct dispatch
let _result = service
    .dispatch("counter.initialize", json!({ "id": "c1" }), Session::new())
    .await?;
```

### Guards

`.guarded(guard, handler)` runs the guard before the handler — if it returns `false`, the command is rejected:

```rust
service
    .command("admin.reset")
    .guarded(
        |ctx: &Context<Repo>| ctx.role() == Some("admin"),
        |_ctx: &Context<Repo>| async { Ok(json!({ "reset": true })) },
    );
```

### Handler File Convention

For larger services, organize handlers into separate files. Each handler module exports a `COMMAND` (or `EVENT` / `EVENTS`) name, a `guard`, and an async `handle`:

```rust
// src/handlers/counter_create.rs
use serde::Deserialize;
use serde_json::{json, Value};
use distributed::microsvc::{Context, HandlerError};
use distributed::OutboxMessage;

use super::Repo;
use crate::models::counter::Counter;

pub const COMMAND: &str = "counter.initialize";

#[derive(Deserialize)]
struct Input { id: String }

pub fn guard(ctx: &Context<Repo>) -> bool {
    ctx.has_fields(&["id"])
}

pub async fn handle(ctx: &Context<'_, Repo>) -> Result<Value, HandlerError> {
    let input = ctx.input::<Input>()?;

    if ctx.repo().get(&input.id).await?.is_some() {
        return Err(HandlerError::Rejected(format!("counter {} already exists", input.id)));
    }

    let mut counter = Counter::default();
    counter.create(input.id.clone())?;

    let message = OutboxMessage::domain_event("counter.initialized", &counter)?;
    ctx.repo().outbox(message).commit(&mut counter).await?;

    Ok(json!({ "id": input.id }))
}
```

Register them with the `register_handlers!` macro:

```rust
let service = distributed::register_handlers!(
    Service::new().with_repo(HashMapRepository::new().queued().aggregate::<Counter>()),
    command handlers::counter_create,
    command handlers::counter_increment,
);
```

Event projection handlers use `EVENT` / `EVENTS` and `event handlers::...` in the same way; inside the handler, `ctx.message()` gives the raw transport `Message` and `ctx.input::<T>()` decodes its payload.

### HTTP Transport (requires `http` feature)

The `http` feature adds an axum-based HTTP transport. Every registered command becomes a `POST /:command` endpoint. Request headers flow into the `Session`:

```rust
use std::sync::Arc;
use distributed::microsvc;

// Get an axum Router to compose with other routes
let app = microsvc::router(service.clone());

// Or serve directly
microsvc::serve(service, "0.0.0.0:3000").await?;
```

Routes:

| Method | Path | Description |
|---|---|---|
| `POST` | `/:command` | Dispatch a command. Body = JSON input, headers = session variables. |
| `GET` | `/health` | Health check: `{ "ok": true, "commands": ["counter.initialize", ...] }` |

```bash
curl -X POST http://localhost:3000/counter.initialize \
  -H 'Content-Type: application/json' \
  -H 'x-hasura-user-id: user-42' \
  -d '{"id": "c1"}'

curl http://localhost:3000/health
```

### gRPC Transport (requires `grpc` feature)

The `grpc` feature adds a tonic-based gRPC transport using standard protobuf wire format (no `.proto` file needed):

```rust
// Get a CommandServiceServer to compose with other tonic routes
let grpc_svc = microsvc::grpc_server(service.clone());

// Or serve directly
microsvc::serve_grpc(service, "[::1]:50051").await?;
```

| RPC | Input | Output | Description |
|---|---|---|---|
| `Dispatch` | `GrpcRequest` | `GrpcResponse` | Dispatch a command. `input` = JSON string, `session_variables` = metadata map. |
| `Health` | `HealthRequest` | `HealthResponse` | Health check. |

Session handling mirrors HTTP — gRPC metadata headers are merged with payload `session_variables` (payload takes precedence). Errors are returned inside `GrpcResponse.status` (HTTP-style status codes), keeping client behavior identical across transports.

### Bus Transport

Attach a bus with `service.with_bus(bus)` and drive it with `run(opts)`: it
derives `listen` (point-to-point commands) and `subscribe` (fan-out events) from
the registered handlers, and makes `repo.outbox(msg).commit(agg)` publish on
commit. The same `Service` can handle commands from multiple transports
simultaneously — HTTP, gRPC, bus, and direct dispatch all share the same handlers
and repository. For finer-grained control, call the `listen` / `subscribe` facade
methods directly. See [Service Bus](#service-bus) above.

### Error Handling

`HandlerError` maps to HTTP-style status codes:

| Variant | Status Code |
|---|---|
| `UnknownCommand` | 404 |
| `DecodeFailed` | 400 |
| `GuardRejected` | 400 |
| `Rejected` | 422 |
| `NotFound` | 404 |
| `Unauthorized` | 401 |
| `Repository` | 500 |
| `Other` | 500 |

## Read Models

Read models are query-optimized relational projections derived from aggregates, event records, or published messages. They are written as declared relational rows using table metadata from `#[derive(ReadModel)]`. Use JSON/JSONB columns for whole-view or semistructured fields.

### Defining a Read Model

```rust
use serde::{Deserialize, Serialize};
use distributed::ReadModel;

#[derive(Clone, Debug, Serialize, Deserialize, ReadModel)]
#[table("game_views")]
pub struct GameView {
    #[id]
    pub id: String,
    pub player_name: String,
    pub score: i32,
    #[jsonb]
    pub metadata: serde_json::Value,
}
```

### Atomic Commits (Read Model + Aggregate)

When the response to a command must include the fully consistent, updated view, commit the aggregate and read model together in one transaction:

```rust
use distributed::{ReadModelWritePlanCommitExt, ReadModelWritePlanBuilder};

// Player submits a move
game.make_move(player_move)?;

// Build the view from the updated aggregate
let view = GameView::from(&game);

// Commit aggregate + view in one transactional batch
let mut read_models = ReadModelWritePlanBuilder::new();
read_models.upsert(&view)?;
repo.read_models(read_models).commit(&mut game).await?;

// Return `view` to the client — it reflects the committed state
```

For related rows, build the same structured write plan:

```rust
let mut read_models = ReadModelWritePlanBuilder::new();
read_models.upsert(&player_view)?;
read_models.upsert_related(&player_view, "weapons", &weapon_view)?;
repo.read_models(read_models).commit(&mut game).await?;
```

This is a deliberate consistency tradeoff: the read model is in sync with the aggregate only when the repository can write both in the same transaction boundary (`TransactionalCommit`). For cross-service or cross-database views, use the eventually consistent outbox/projector pattern instead.

### Eventual Projection

Distributed projectors subscribe to published messages and commit read-model rows through a workspace, marking the message processed in the same adapter transaction for SQL idempotency:

```rust
use distributed::ReadModelWorkspaceExt;

let mut workspace = ctx.read_model_store().workspace();
workspace.upsert(&row)?;
workspace.commit().await?;
```

### Loading

```rust
use distributed::{ReadModelWorkspaceExt, RowKey, RowValue};

let loaded = repo
    .workspace()
    .load::<GameView>(RowKey::new([("id", RowValue::String("view-1".into()))]))
    .one()
    .await?;
```

See [`docs/read-models.md`](docs/read-models.md) for the full guide, including relational metadata, schema bootstrap, relationship includes, distributed idempotency, and non-goals.

## Snapshots

As aggregates accumulate events, replaying from scratch gets expensive. The framework keeps aggregate events as the durable source of truth and stores repository snapshots as a rebuildable hydration cache. A snapshot cache record can be deleted and rebuilt from events without changing aggregate correctness.

### Making an Aggregate Snapshottable

Add `#[derive(Snapshot)]` to your aggregate struct. This generates a state snapshot payload DTO (e.g. `TodoSnapshot`), a `fn snapshot()` method, and the full `impl Snapshottable`:

```rust
use distributed::{Entity, Snapshot};

#[derive(Default, Snapshot)]
struct Todo {
    entity: Entity,
    user_id: String,
    task: String,
    completed: bool,
}
```

Fields with `#[serde(skip)]` (like `emitter: EntityEmitter`) are automatically excluded.

**Custom ID key** — when the entity ID maps to a domain field like `sku`:

```rust
#[derive(Default, Snapshot)]
#[snapshot(id = "sku")]
struct Inventory {
    entity: Entity,
    sku: String,
    available: u32,
}
```

**Custom entity field name**:

```rust
#[derive(Default, Snapshot)]
#[snapshot(entity = "my_entity")]
struct Widget {
    my_entity: Entity,
    name: String,
}
```

### Using Snapshots

Chain `.with_snapshots(frequency)` onto any aggregate repository. The frequency is how many events between automatic snapshots:

```rust
use distributed::{AggregateBuilder, HashMapRepository, Queueable, RepositoryError};

let repo = HashMapRepository::new()
    .queued()
    .aggregate::<Todo>()
    .with_snapshots(10); // snapshot every 10 events

// Commit works normally — snapshots are created automatically at the threshold
let mut todo = Todo::default();
todo.initialize("todo-1".into(), "user-1".into(), "Ship it".into())?;
repo.commit(&mut todo).await?;

// Load transparently restores from the latest snapshot + replays newer events
let Some(todo) = repo.get("todo-1").await? else {
    return Err(RepositoryError::NotFound { id: "todo-1".into() });
};
```

### How It Works

- **On commit**: If `entity.version().saturating_sub(snapshot_version) >= frequency`, the aggregate's state is serialized via `create_snapshot()` and staged into the same commit transaction as the event append.
- **On load**: If a usable snapshot cache record exists, the aggregate is restored from its payload and only events with `sequence > snapshot.version` are replayed. Invalid, incompatible, or ahead-of-stream cache records fall back to full replay.
- **Storage**: Snapshot cache records are stored separately from the event stream, keyed by full stream identity. They carry aggregate type, aggregate ID, covered event version, snapshot payload type/version, codec metadata, cache metadata, and timestamp.

## Event Upcasting / Versioning

Event schemas evolve over time. When you add a field to an event (e.g., `priority` to `Initialized`), old serialized events in storage can't deserialize into the new type. **Upcasters** solve this: typed functions that transform old event payload shapes into the current format at read time, without modifying stored data.

### Defining an Upcaster

An upcaster is a plain function that converts a typed payload from one version to the next. The crate handles payload decoding and encoding:

```rust
type InitV1 = (String, String);
type InitV2 = (String, String, u8);

/// Upcasts Initialized v1 (id, task) → v2 (id, task, priority)
fn upcast_init_v1_v2((id, task): InitV1) -> InitV2 {
    (id, task, 0)
}
```

### Registering Upcasters

With `#[sourced]`, add upcasters directly in the attribute:

```rust
#[sourced(entity, upcasters(
    ("initialized", 1 => 2, InitV1 => InitV2, upcast_init_v1_v2),
))]
impl Todo {
    #[event("initialized", version = 2)]
    fn initialize(&mut self, id: String, task: String, priority: u8) {
        self.entity.set_id(&id);
        self.task = task;
        self.priority = priority;
    }

    #[event("completed", when = !self.completed)]
    fn complete(&mut self) {
        self.completed = true;
    }
}
```

Old events stored as `(id, task)` at v1 are transparently upcast to `(id, task, 0u8)` at v2 during hydration. New events are created at v2 via the `version = 2` parameter on `#[event]`.

### Chaining Upcasters

Upcasters chain automatically. Each transforms one version to the next (v1→v2→v3):

```rust
#[sourced(entity, upcasters(
    ("initialized", 1 => 2, InitV1 => InitV2, upcast_init_v1_v2),
    ("initialized", 2 => 3, InitV2 => InitV3, upcast_init_v2_v3),
))]
impl Todo { /* ... */ }
```

A v1 event automatically chains through v1→v2→v3; a v2 event only goes through v2→v3; a v3 event passes through unchanged.

### How It Works

- **On hydrate**: Before replaying events, the aggregate's registered upcasters are applied by event name and version.
- **On snapshot hydrate**: Only post-snapshot events are upcast — the snapshot already contains the current state.
- **No stored data modified**: Upcasters are read-time transformations.
- **Zero overhead when unused**: Aggregates with no upcasters take the fast hydration path.

## Project Structure

```
src/
  aggregate/      # Aggregate trait, hydration, async aggregate repository helpers
  commit_builder/ # Async transactional batches for aggregates, outbox, and read models
  emitter/        # In-process event emitter helpers (feature = "emitter")
  entity/         # Entity, event records, metadata, upcasting codecs
  hashmap_repo/   # In-memory repository (implements every async trait)
  lock/           # Async lock + lock manager traits, in-memory locks
  microsvc/       # Command/event handler framework: service, context, session
    transport/    # Bus facade + adapters (in-memory, postgres, nats, rabbitmq, kafka, knative)
  outbox/         # Durable outbox message + commit extension
  outbox_worker/  # Outbox claiming, publishing, workers
  postgres_repo/  # Postgres async SQL repository (feature = "postgres")
  queued_repo/    # Async queue-based locking repository wrapper
  read_model/     # Read model store traits, in-memory store, schema metadata
  snapshot/       # Snapshot store traits, in-memory store, snapshot repository
  sqlite_repo/    # SQLite async SQL repository (feature = "sqlite")
  table/          # Neutral table/row primitives shared by read models and ops tables
  lib.rs          # Public exports
distributed_macros/
  src/            # Proc macros: sourced, digest, aggregate, enqueue, ReadModel, Snapshot
docs/
  repositories.md
  async-transports.md
  read-models.md
  postgres-event-store.md
  research-and-roadmap.md
migrations/       # Explicit SQLite and Postgres migrations
compose.yaml      # Local postgres / rabbitmq / kafka / nats for integration tests
```

## Running Tests

```bash
cargo test                  # default features (`emitter`)
cargo test --features http
cargo test --features grpc
make test                 # starts compose and runs full local coverage
cargo test --all-features   # all features; broker tests skip without env vars
```

### Real-Broker Integration Tests

The transport adapters have integration tests against real brokers. They are feature-gated and **skip when their env var is unset**:

```bash
docker compose up -d   # postgres, rabbitmq, kafka, nats (see compose.yaml)

DATABASE_URL=postgres://sourced:sourced@localhost:5432/distributed \
  cargo test --test postgres_transport --features postgres
NATS_URL=nats://localhost:4222 \
  cargo test --test nats_transport --features nats
AMQP_URL=amqp://guest:guest@localhost:5672/%2f \
  cargo test --test rabbitmq_transport --features rabbitmq
KAFKA_BROKERS=127.0.0.1:9092 \
  cargo test --test kafka_transport --features kafka
```

Each broker has a matching reusable GitHub Actions job (`.github/workflows/integration-*.yaml`) that runs on PRs and on push to `main`.

## Coverage Reporting

This project uses [`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov):

```bash
rustup component add llvm-tools-preview
cargo install cargo-llvm-cov

cargo llvm-cov --all-features --summary-only
cargo llvm-cov --all-features --lcov --output-path lcov.info
```

CI also publishes `lcov.info` as a workflow artifact and attempts an optional Codecov upload.

## Examples

- `tests/sourced/` — `#[sourced]` macro with typed event enum, `TryFrom`, and aggregate hydration
- `tests/sourced_upcasting/` — `#[sourced]` with upcasters (v1→v2→v3 chains)
- `tests/sourced_enqueue/` — `#[sourced(entity, enqueue)]` integrated choreography
- `tests/sourced_snapshot/` — `#[derive(Snapshot)]` with custom ID keys, `serde(skip)` exclusion, and custom entity fields
- `tests/snapshots/` — snapshot creation, loading, and partial replay
- `tests/upcasting/` — event versioning with v1→v2→v3 upcasters, chaining, and snapshot integration
- `tests/read_models/` — relational read-model projections and atomic commits
- `tests/distributed_read_model/` — multi-service projection over the bus + persistence matrix
- `tests/microsvc/` — async handlers, dispatch, session, convention, HTTP, gRPC, and bus transports
- `tests/sagas/` — saga orchestration and choreography with the outbox pattern
- `tests/sqlite_repository/`, `tests/postgres_repository/` — durable SQL adapters
- `tests/transport_conformance/`, `tests/{nats,rabbitmq,kafka,postgres}_transport/`, `tests/knative_cloudevents/` — transport adapters and the shared conformance harness

## License

MIT. See `LICENSE`.
