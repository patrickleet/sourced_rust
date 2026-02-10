# Sourced Rust

Sourced Rust started as a small event-sourcing toolkit for Rust before expanding to become a full CQRS/ES+AR framework. 

It keeps your domain model as a plain struct (PORS), inspired by POCO/POJO, while giving you append-only events, replay, and persistence.

It also provides you with tools for producing and consuming events for use locally in a multi-threaded process, or distributed across networks.

It is built with stateless vertical and horizontal scaling in Cloud Native environments in mind, and can be used to build a single service that can easily be broken into many later for partition based scaling.

## Project Inspiration

Sourced Rust is inspired by the original [sourced](https://github.com/mateodelnorte/sourced) Node.js project by Matt Walters and his accompanying [servicebus](https://github.com/mateodelnorte/servicebus) library for distributed messaging. Patrick Lee Scott, a contributor and maintainer of the original JavaScript/TypeScript versions, brought these concepts to Rust and refactored them for the Rust ecosystem.

## Design Goals

- Keep domain objects simple and explicit (Plain Old Rust Structs).
- Make events the source of truth for state.
- Make replay predictable and safe.
- Keep storage pluggable and testable.
- Add optional queue-based locking for serialized workflows.

## Quick Start

```rust
use serde::{Deserialize, Serialize};
use sourced_rust::{
    aggregate, digest, AggregateBuilder, Entity, HashMapRepository,
    OutboxCommitExt, OutboxMessage, Queueable,
};

#[derive(Default)]
struct Todo {
    entity: Entity,
    user_id: String,
    task: String,
    completed: bool,
}

impl Todo {
    #[digest("Initialized")]
    fn initialize(&mut self, id: String, user_id: String, task: String) {
        self.entity.set_id(&id);
        self.user_id = user_id;
        self.task = task;
    }

    #[digest("Completed", when = !self.completed)]
    fn complete(&mut self) {
        self.completed = true;
    }
}

aggregate!(Todo, entity {
    "Initialized"(id, user_id, task) => initialize,
    "Completed"() => complete(),
});

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let repo = HashMapRepository::new().queued().aggregate::<Todo>();

    let mut todo = Todo::default();
    todo.initialize("todo-1".into(), "user-1".into(), "Ship it".into());
    repo.commit(&mut todo)?;

    if let Some(mut todo) = repo.get("todo-1")? {
        todo.complete();
        repo.commit(&mut todo)?;
    }

    Ok(())
}
```

## Core Concepts

- **Entity**: Holds the event history. You embed it in your domain structs.
- **EventRecord**: An immutable event with name, payload, sequence, and timestamp.
- **Repository**: Persists and loads entities by event history.
- **HashMapRepository**: In-memory repository for tests and examples.
- **QueuedRepository**: Wraps any repository and adds per-entity queue locking.
- **Snapshottable**: Opt-in trait for aggregates that support periodic snapshots for fast hydration.
- **SnapshotAggregateRepository**: Wraps an `AggregateRepository` to transparently create and load snapshots.
- **OutboxMessage**: A durable integration event for the outbox pattern. Supports optional `destination` for point-to-point routing.
- **Outbox Worker**: Publishes outbox messages to external systems. `spawn` for fan-out, `spawn_routed` for point-to-point routing.
- **Bus**: Service bus with two patterns: `publish/subscribe` (fan-out) and `send/listen` (point-to-point).
- **EventReceiver**: Filtered subscription that only receives specified event types.

## Pluggable by Default

Every infrastructure concern in `sourced_rust` follows the same pattern: a **trait** defines the contract, an **in-memory implementation** ships out of the box for testing and development, and you swap in your own for production.

| Concern | Trait | In-memory default | Swap in for production |
|---|---|---|---|
| Storage | `Repository` (`Get + Find + Commit + ...`) | `HashMapRepository` | Postgres, DynamoDB, etc. |
| Messaging | `Publisher` + `Subscriber` | `InMemoryQueue` | Kafka, Redis Streams, SQS, etc. |
| Read model store | `ReadModelStore` | `InMemoryReadModelStore` | Postgres, MongoDB, etc. |
| Snapshot store | `SnapshotStore` | `InMemorySnapshotStore` | Postgres, S3, etc. |
| Outbox publishing | `OutboxPublisher` | `LogPublisher` | Any `Publisher` impl |
| Locking | `Lock` + `LockManager` | `InMemoryLockManager` | Redis, Postgres advisory, etc. |

All in-memory defaults are `Clone` and `Send + Sync`, so they work in single-threaded tests and multi-threaded servers alike. When you're ready for production, implement the trait for your infrastructure and plug it in — no other code changes needed.

## The `#[digest]` Macro

The `#[digest]` attribute macro automatically records events when methods are called.

**Basic usage** - captures function parameters:

```rust
#[digest("Initialized")]
fn initialize(&mut self, id: String, user_id: String, task: String) {
    self.entity.set_id(&id);
    self.user_id = user_id;
    self.task = task;
}
```

**Guard conditions** - only emit when condition is true:

```rust
#[digest("Completed", when = !self.completed)]
fn complete(&mut self) {
    self.completed = true;
}
```

**Custom entity field** - when your entity field isn't named `entity`:

```rust
#[digest(my_entity, "Created")]
fn create(&mut self, name: String) {
    // uses self.my_entity instead of self.entity
}
```

## The `aggregate!` Macro

Generates the `Aggregate` trait implementation with replay logic:

```rust
aggregate!(Todo, entity {
    "Initialized"(id, user_id, task) => initialize,
    "Completed"() => complete(),
});
```

This generates:
- `impl Aggregate for Todo` with `entity()`, `entity_mut()`, and `replay_event()`

## In-Process Event Choreography (requires `emitter` feature)

The `emitter` feature (enabled by default) adds in-process event-driven choreography — queue local events during commands and emit them after commit for reactive workflows within a process.

### The `#[enqueue]` Macro

Works alongside `#[digest]` — `#[digest]` records the event to the entity's stream for replay, while `#[enqueue]` queues a local copy for in-process emission. You'll typically use both together:

```rust
use sourced_rust::{aggregate, digest, enqueue, Entity};
use sourced_rust::emitter::EntityEmitter;

#[derive(Default)]
struct OrderSaga {
    entity: Entity,
    #[serde(skip, default)]
    emitter: EntityEmitter,
    order_id: String,
    status: String,
}

impl OrderSaga {
    #[digest("OrderStarted")]
    #[enqueue("OrderStarted")]
    fn start(&mut self, order_id: String) {
        self.entity.set_id(&order_id);
        self.order_id = order_id;
        self.status = "started".into();
    }

    #[digest("StepCompleted", when = self.status == "started")]
    #[enqueue("StepCompleted", when = self.status == "started")]
    fn complete_step(&mut self) {
        self.status = "completed".into();
    }
}

aggregate!(OrderSaga, entity {
    "OrderStarted"(order_id) => start,
    "StepCompleted"() => complete_step(),
});
```

**Custom emitter field** — when your emitter field isn't named `emitter`:

```rust
#[enqueue(my_emitter, "Created")]
fn create(&mut self, name: String) {
    // uses self.my_emitter instead of self.emitter
}
```

### Emitting After Commit

Queued events are held until you explicitly emit them after a successful commit:

```rust
let mut saga = OrderSaga::default();
saga.start("order-1".into());

// Commit the aggregate...
repo.commit(&mut saga)?;

// Then emit queued events to registered listeners
saga.emitter.emit_queued();
```

### Registering Listeners

Register callbacks that fire when events are emitted:

```rust
let shared_state = Arc::new(Mutex::new(Vec::new()));
let state = Arc::clone(&shared_state);

saga.emitter.on("OrderStarted", move |payload: String| {
    state.lock().unwrap().push(payload);
});
```

This pattern is useful for reactive workflows where one aggregate's events trigger actions in other aggregates or services within the same process. For cross-service messaging, use the [Outbox Pattern](#outbox-pattern) and [Service Bus](#service-bus) instead.

## Queued Repository

Per-entity locking for serialized workflows:

```rust
let repo = HashMapRepository::new().queued().aggregate::<Todo>();

let mut todo = repo.get("todo-1")?.unwrap(); // locks this ID
// ... mutate ...
repo.commit(&mut todo)?; // unlocks

// Or release without changes:
repo.abort(&todo)?;

// Read without locking:
let _ = repo.peek("todo-1")?;
```

By default, locking is in-memory. For distributed deployments, plug in a custom `LockManager`:

```rust
let redis_locks = MyRedisLockManager::new(/* ... */);
let repo = HashMapRepository::new()
    .queued_with(redis_locks)
    .aggregate::<Todo>();
```

## Outbox Pattern

Each outbox message is its own aggregate, committed alongside your domain entity:

```rust
use sourced_rust::{OutboxCommitExt, OutboxMessage};

let mut todo = Todo::default();
todo.initialize("todo-1".into(), "user-1".into(), "Buy milk".into());

let mut message = OutboxMessage::encode(
    format!("{}:init", todo.entity.id()),
    "TodoInitialized",
    &todo.snapshot(),
)?;

// Commit both atomically
repo.outbox(&mut message).commit(&mut todo)?;
```

### Outbox Worker

A separate process claims and publishes pending messages:

```rust
use sourced_rust::{LogPublisher, OutboxRepositoryExt, OutboxWorker};
use std::time::Duration;

let repo = HashMapRepository::new();
let mut worker = OutboxWorker::new(LogPublisher::new());

let mut claimed = repo.claim_outbox_messages("worker-1", 100, Duration::from_secs(30))?;
let _ = worker.process_batch(&mut claimed);

for message in &mut claimed {
    repo.commit(&mut message.entity)?;
}
```

## Service Bus

The service bus supports two messaging patterns:

- **Publish/Subscribe (fan-out)**: Every subscriber receives every event. Use `publish()` / `subscribe()`.
- **Send/Listen (point-to-point)**: Each message goes to a named queue where only one listener consumes it. Use `send()` / `listen()`.

### Publish/Subscribe (Fan-Out)

```rust
use sourced_rust::bus::{Bus, InMemoryQueue, Publisher, Event};

let queue = InMemoryQueue::new();
let bus = Bus::from_queue(queue);

// Publish events (all subscribers see them)
bus.publish(Event::with_string_payload("evt-1", "OrderCreated", r#"{"id":"123"}"#))?;
```

### Filtered Subscriptions

Subscribe to specific event types with `bus.subscribe()`. Each subscriber only receives events matching its subscribed types:

```rust
use sourced_rust::bus::{Bus, InMemoryQueue};

let queue = InMemoryQueue::new();
let bus = Bus::from_queue(queue);

// Subscribe to specific event types
let order_events = bus.subscribe(&["OrderCreated", "OrderCompleted"]);
let payment_events = bus.subscribe(&["PaymentSucceeded", "PaymentFailed"]);

// Each receiver only gets its subscribed events
while let Ok(Some(event)) = order_events.recv(100) {
    match event.event_type.as_str() {
        "OrderCreated" => { /* handle */ }
        "OrderCompleted" => { /* handle */ }
        _ => unreachable!(),
    }
}
```

### Send/Listen (Point-to-Point)

Send messages to named queues. Each message is consumed by exactly one listener (competing consumers):

```rust
use sourced_rust::bus::{Bus, InMemoryQueue, Event};

let queue = InMemoryQueue::new();
let bus = Bus::from_queue(queue);

// Send to a named queue
bus.send("orders", Event::with_string_payload("evt-1", "ProcessOrder", r#"{"id":"123"}"#))?;

// Listen on a named queue (blocks until message or timeout)
if let Ok(Some(event)) = bus.listen("orders", 1000) {
    // Only one listener gets each message
}
```

### Distributed Services (Fan-Out)

For multi-threaded or distributed scenarios with fan-out, each service creates its own `Bus` from a shared queue and subscribes to event types:

```rust
use std::thread;
use sourced_rust::bus::{Bus, InMemoryQueue};

let queue = InMemoryQueue::new();

// Order Service thread
let order_queue = queue.clone();
thread::spawn(move || {
    let bus = Bus::from_queue(order_queue);
    let events = bus.subscribe(&["SagaStarted"]);

    while let Ok(Some(event)) = events.recv(1000) {
        // Handle SagaStarted events
    }
});

// Payment Service thread
let payment_queue = queue.clone();
thread::spawn(move || {
    let bus = Bus::from_queue(payment_queue);
    let events = bus.subscribe(&["InventoryReserved"]);

    while let Ok(Some(event)) = events.recv(1000) {
        // Handle InventoryReserved events
    }
});
```

### Distributed Services (Point-to-Point)

For point-to-point messaging, each service listens on its own named queue:

```rust
use std::thread;
use sourced_rust::bus::{Bus, InMemoryQueue};

let queue = InMemoryQueue::new();

// Order Service listens on its own queue
let order_queue = queue.clone();
thread::spawn(move || {
    let bus = Bus::from_queue(order_queue);

    while let Ok(Some(event)) = bus.listen("orders", 1000) {
        // Handle messages sent to the "orders" queue
    }
});

// Payment Service listens on its own queue
let payment_queue = queue.clone();
thread::spawn(move || {
    let bus = Bus::from_queue(payment_queue);

    while let Ok(Some(event)) = bus.listen("payments", 1000) {
        // Handle messages sent to the "payments" queue
    }
});
```

### With Outbox Worker (Fan-Out)

Combine the outbox pattern with the service bus for reliable event publishing:

```rust
use sourced_rust::{
    bus::Bus, HashMapRepository, InMemoryQueue, OutboxWorkerThread,
    OutboxCommitExt, OutboxMessage, AggregateBuilder, Queueable,
};
use std::time::Duration;

let queue = InMemoryQueue::new();
let repo = HashMapRepository::new();

// Start outbox worker - publishes messages to the queue (fan-out)
let worker = OutboxWorkerThread::spawn(
    repo.clone(),
    queue.clone(),
    Duration::from_millis(100),
);

// Create bus for this service
let bus = Bus::from_queue(queue);
let order_repo = repo.queued().aggregate::<Order>();

// Commit entity with outbox message
let mut order = Order::new();
order.create("order-1".into(), "customer-1".into());

let mut outbox = OutboxMessage::encode(
    "order-1:created",
    "OrderCreated",
    &OrderCreatedPayload { order_id: "order-1".into() },
)?;
order_repo.outbox(&mut outbox).commit(&mut order)?;

// Other services receive the event via their subscriptions
let events = bus.subscribe(&["OrderCreated"]);
```

### With Outbox Worker (Point-to-Point)

Use `OutboxMessage::encode_to()` to set a destination queue, and `spawn_routed` to route messages:

```rust
use sourced_rust::{
    CommitBuilderExt, HashMapRepository, InMemoryQueue, OutboxWorkerThread,
    OutboxMessage,
};
use std::time::Duration;

let queue = InMemoryQueue::new();
let repo = HashMapRepository::new();

// spawn_routed: sends to named queues when destination is set,
// falls back to publish (fan-out) when destination is None
let worker = OutboxWorkerThread::spawn_routed(
    repo.clone(),
    queue.clone(),
    Duration::from_millis(100),
);

// Create outbox messages with destinations
let outbox_saga = OutboxMessage::encode_to(
    "order-1:created:saga",
    "OrderCreated",
    "saga",   // destination queue
    &payload,
)?;
let outbox_inventory = OutboxMessage::encode_to(
    "order-1:created:inventory",
    "OrderCreated",
    "inventory",  // destination queue
    &payload,
)?;

// Commit aggregate + multiple outbox messages atomically
repo.outbox(outbox_saga)
    .outbox(outbox_inventory)
    .commit(&mut order)?;

// Worker drains outbox and sends each message to its destination queue
```

## Read Models

Read models are denormalized views derived from event-sourced aggregates. They give you fast, purpose-built query models shaped for your UI or API consumers.

### Defining a Read Model

```rust
use serde::{Deserialize, Serialize};
use sourced_rust::ReadModel;

#[derive(Clone, Debug, Serialize, Deserialize, ReadModel)]
#[readmodel(collection = "game_views")]
pub struct GameView {
    #[readmodel(id)]
    pub id: String,
    pub player_name: String,
    pub score: i32,
}
```

### Atomic Commits (Read Model + Aggregate)

When the response to a command must include the fully consistent, updated view, you can commit the aggregate and read model together:

```rust
use sourced_rust::CommitBuilderExt;

// Player submits a move
game.make_move(player_move);

// Build the view from the updated aggregate
let view = GameView::from(&game);

// Commit aggregate + view atomically
repo.readmodel(&view).commit(&mut game)?;

// Return `view` to the client — it reflects the committed state
```

This is a deliberate CAP theorem tradeoff: you're choosing **consistency** over **partition tolerance**. The read model is always in sync with the aggregate because they're written in the same transaction, but this only works within a single process against a single store. For cross-service or cross-database views, use the eventually consistent outbox pattern instead.

See [`docs/read-models.md`](docs/read-models.md) for the full guide, including eventually consistent projections, `QueuedReadModelStore`, and a decision flowchart.

## Snapshots

As aggregates accumulate events, replaying from scratch gets expensive. Snapshots let you periodically capture an aggregate's state and restore from it, replaying only the events that came after.

### Making an Aggregate Snapshottable

Implement the `Snapshottable` trait on your aggregate:

```rust
use sourced_rust::Snapshottable;

impl Snapshottable for Todo {
    type Snapshot = TodoSnapshot;

    fn create_snapshot(&self) -> TodoSnapshot {
        self.snapshot()
    }

    fn restore_from_snapshot(&mut self, s: TodoSnapshot) {
        self.entity.set_id(&s.id);
        self.user_id = s.user_id;
        self.task = s.task;
        self.completed = s.completed;
    }
}
```

### Using Snapshots

Chain `.with_snapshots(frequency)` onto any `AggregateRepository`. The frequency is how many events between automatic snapshots:

```rust
let repo = HashMapRepository::new()
    .queued()
    .aggregate::<Todo>()
    .with_snapshots(10); // snapshot every 10 events

// Commit works normally — snapshots are created automatically at the threshold
let mut todo = Todo::default();
todo.initialize("todo-1".into(), "user-1".into(), "Ship it".into());
repo.commit(&mut todo)?;

// Load transparently restores from latest snapshot + replays newer events
let todo = repo.get("todo-1")?.unwrap();
```

### How It Works

- **On commit**: If `entity.version() >= snapshot_version + frequency`, the aggregate's state is serialized via `create_snapshot()` and saved to the snapshot store.
- **On load**: If a snapshot exists, the aggregate is restored from it and only events with `sequence > snapshot.version` are replayed. If no snapshot exists, full replay is used as a fallback.
- **Storage**: Snapshots are stored separately from the event stream. `HashMapRepository` embeds an `InMemorySnapshotStore`; for production, implement the `SnapshotStore` trait for your backend.

## Project Structure

```
src/
  core/       # Entity, events, repository traits, aggregate helpers
  bus/        # Service bus, publishers, subscribers
  emitter/    # In-process event emitter helpers
  hashmap/    # In-memory repository
  lock/       # Lock trait, LockManager trait, InMemoryLock
  queued/     # Queue-based locking wrapper
  read_model/ # Read model store traits and InMemoryReadModelStore
  snapshot/   # Snapshot store traits, InMemorySnapshotStore, SnapshotAggregateRepository
  outbox/     # Outbox message aggregate + worker + publishers
  lib.rs      # Public exports
```

## Running Tests

```
cargo test
```

## Examples

- `tests/todos/` - Basic entity workflow
- `tests/snapshots/` - Snapshot creation, loading, and partial replay
- `tests/sagas/distributed.rs` - Multi-service saga with outbox pattern (fan-out and point-to-point)
- `tests/sagas/orchestration.rs` - Saga orchestration with compensation

## License

MIT. See `LICENSE`.
