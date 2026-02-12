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

### Define the Aggregate

```rust
use sourced_rust::{sourced, Entity, Snapshot};

#[derive(Default, Snapshot)]
struct Todo {
    entity: Entity,
    user_id: String,
    task: String,
    completed: bool,
}

#[sourced(entity)]
impl Todo {
    #[event("Initialized")]
    fn initialize(&mut self, id: String, user_id: String, task: String) {
        self.entity.set_id(&id);
        self.user_id = user_id;
        self.task = task;
    }

    #[event("Completed", when = !self.completed)]
    fn complete(&mut self) {
        self.completed = true;
    }
}

// #[sourced] generates: TodoEvent enum, TryFrom<&EventRecord>, impl Aggregate
// #[derive(Snapshot)] generates: TodoSnapshot struct, fn snapshot(), impl Snapshottable
```

### Define Command Handlers

Each handler is a module with `COMMAND`, `guard`, and `handle`:

```rust
// handlers/todo_create.rs
use serde::Deserialize;
use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::{AggregateBuilder, OutboxCommitExt, OutboxMessage, Repository};

pub const COMMAND: &str = "todo.create";

#[derive(Deserialize)]
struct Input { id: String, user_id: String, task: String }

pub fn guard<R>(ctx: &Context<R>) -> bool {
    ctx.has_fields(&["id", "user_id", "task"])
}

pub fn handle<R: Repository + Clone>(ctx: &Context<R>) -> Result<Value, HandlerError> {
    let input = ctx.input::<Input>()?;
    let repo = ctx.repo().clone().aggregate::<Todo>();

    let mut todo = Todo::default();
    todo.initialize(input.id.clone(), input.user_id, input.task);

    // Outbox message derives id, snapshot payload, and metadata automatically
    let mut outbox = OutboxMessage::domain_event("TodoInitialized", &todo)
        .map_err(|e| HandlerError::Other(Box::new(e)))?;
    repo.outbox(&mut outbox).commit(&mut todo)?;

    Ok(json!({ "id": input.id }))
}
```

### Wire It Up

```rust
use std::sync::Arc;
use sourced_rust::{microsvc, HashMapRepository, Queueable};

fn main() {
    let service = Arc::new(sourced_rust::register_handlers!(
        microsvc::Service::new(HashMapRepository::new().queued()),
        handlers::todo_create,
        handlers::todo_complete,
    ));

    // Direct dispatch
    let result = service.dispatch(
        "todo.create",
        serde_json::json!({ "id": "todo-1", "user_id": "alice", "task": "Ship it" }),
        microsvc::Session::new(),
    );

    // Or serve over HTTP (requires `http` feature)
    // microsvc::serve(service, "0.0.0.0:3000").await?;
}
```

## Core Concepts

- **Entity**: Holds the event history. You embed it in your domain structs.
- **EventRecord**: An immutable event with name, payload, sequence, timestamp, and optional metadata.
- **Repository**: Persists and loads entities by event history.
- **HashMapRepository**: In-memory repository for tests and examples.
- **QueuedRepository**: Wraps any repository and adds per-entity queue locking.
- **EventUpcaster**: A pure, stateless transformation that converts event payloads from one version to another at read time.
- **Snapshottable**: Opt-in trait for aggregates that support periodic snapshots for fast hydration. Use `#[derive(Snapshot)]` to auto-generate the snapshot struct and trait impl.
- **SnapshotAggregateRepository**: Wraps an `AggregateRepository` to transparently create and load snapshots.
- **OutboxMessage**: A durable integration event for the outbox pattern. Supports optional `destination` for point-to-point routing and metadata propagation.
- **Outbox Worker**: Publishes outbox messages to external systems. `spawn` for fan-out, `spawn_routed` for point-to-point routing.
- **Bus**: Service bus with two patterns: `publish/subscribe` (fan-out) and `send/listen` (point-to-point).
- **EventReceiver**: Filtered subscription that only receives specified event types.
- **microsvc::Service**: Convention-based command handler framework with pluggable transports (HTTP, bus, direct dispatch).

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

## The `#[sourced]` Macro

The `#[sourced]` attribute macro is the recommended way to define event-sourced aggregates. Place it on an impl block and annotate command methods with `#[event("Name")]`. It replaces both `#[digest]` and `aggregate!()`, and auto-generates a typed event enum.

### Basic Usage

```rust
use sourced_rust::{sourced, Entity};

#[derive(Default)]
struct Todo {
    entity: Entity,
    user_id: String,
    task: String,
    completed: bool,
}

#[sourced(entity)]
impl Todo {
    #[event("Initialized")]
    fn initialize(&mut self, id: String, user_id: String, task: String) {
        self.entity.set_id(&id);
        self.user_id = user_id;
        self.task = task;
    }

    #[event("Completed", when = !self.completed)]
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

// event_name() accessor
impl TodoEvent {
    pub fn event_name(&self) -> &'static str { /* ... */ }
}

// Convert stored events to typed enum
impl TryFrom<&EventRecord> for TodoEvent { /* ... */ }

// Full Aggregate trait impl (entity accessors + replay logic)
impl Aggregate for Todo { /* ... */ }
```

### Using the Typed Event Enum

The generated enum enables exhaustive matching — if you add or remove an event, the compiler tells you everywhere that needs updating:

```rust
let record: &EventRecord = &todo.entity.events()[0];
let event = TodoEvent::try_from(record).unwrap();

match event {
    TodoEvent::Initialized { id, user_id, task } => {
        println!("Todo {} created by {}: {}", id, user_id, task);
    }
    TodoEvent::Completed => {
        println!("Todo completed");
    }
}
```

### Custom Enum Name

Override the default `{StructName}Event` naming:

```rust
#[sourced(entity, events = "TodoCommand")]
impl Todo {
    // generates TodoCommand enum instead of TodoEvent
}
```

### Versioned Events

Create events at a specific version for [upcasting](#event-upcasting--versioning):

```rust
#[sourced(entity, upcasters(
    ("Initialized", 1 => 2, upcast_init_v1_v2),
))]
impl TodoV2 {
    #[event("Initialized", version = 2)]
    fn initialize(&mut self, id: String, task: String, priority: u8) {
        // creates events at version 2
    }

    #[event("Completed", when = !self.completed)]
    fn complete(&mut self) {
        self.completed = true;
    }
}
```

### Custom Entity Field

When your entity field isn't named `entity`:

```rust
#[sourced(my_entity)]
impl MyAggregate {
    #[event("Created")]
    fn create(&mut self, name: String) {
        // uses self.my_entity
    }
}
```

### With `enqueue` for Choreography

Add `enqueue` to `#[sourced]` to automatically queue events for in-process emission alongside digest. No need for separate `#[enqueue]` attributes — every `#[event]` method both records to the entity stream and enqueues for emission:

```rust
use sourced_rust::{sourced, Entity};
use sourced_rust::emitter::EntityEmitter;

struct Order {
    entity: Entity,
    emitter: EntityEmitter,
    status: String,
}

#[sourced(entity, enqueue)]
impl Order {
    #[event("OrderCreated")]
    fn create(&mut self, order_id: String, customer: String) {
        self.entity.set_id(&order_id);
        self.status = "created".into();
    }

    #[event("OrderShipped", when = self.status == "created")]
    fn ship(&mut self) {
        self.status = "shipped".into();
    }
}
```

**Custom emitter field** — when your emitter field isn't named `emitter`:

```rust
#[sourced(entity, enqueue(my_emitter))]
impl Notifier {
    #[event("NotificationSent")]
    fn send(&mut self, id: String, message: String) {
        self.entity.set_id(&id);
        self.message = message;
    }
}
```

## The `#[digest]` Macro and `aggregate!()` Macro

The `#[digest]` and `aggregate!()` macros are the lower-level building blocks that `#[sourced]` replaces. They're still fully supported and useful when you want more granular control.

### The `#[digest]` Macro

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

**Versioned events** - create events at a specific version (see [Event Upcasting](#event-upcasting--versioning)):

```rust
#[digest("Initialized", version = 2)]
fn initialize(&mut self, id: String, task: String, priority: u8) {
    // creates the event at version 2
}
```

**Custom entity field** - when your entity field isn't named `entity`:

```rust
#[digest(my_entity, "Created")]
fn create(&mut self, name: String) {
    // uses self.my_entity instead of self.entity
}
```

### The `aggregate!` Macro

Generates the `Aggregate` trait implementation with replay logic:

```rust
aggregate!(Todo, entity {
    "Initialized"(id, user_id, task) => initialize,
    "Completed"() => complete(),
});
```

With [upcasters](#event-upcasting--versioning) for event schema evolution:

```rust
aggregate!(Todo, entity {
    "Initialized"(id, task, priority) => initialize,
    "Completed"() => complete(),
} upcasters [
    ("Initialized", 1 => 2, upcast_initialized_v1_v2),
]);
```

This generates:
- `impl Aggregate for Todo` with `entity()`, `entity_mut()`, `replay_event()`, and optionally `upcasters()`

## Event Metadata

Metadata lets you attach cross-cutting context — correlation IDs, causation IDs, user context, trace spans — to events without changing your domain model.

### Setting Metadata on an Entity

Set metadata on the entity before calling command methods. Every event produced by `#[digest]` or `#[event]` (within `#[sourced]`) automatically inherits it:

```rust
let mut todo = Todo::default();

// Set context before commands
todo.entity.set_correlation_id("req-abc-123");
todo.entity.set_causation_id("cmd-create-todo");
todo.entity.set_meta("user_id", "u-42");

// Events produced by this call carry the metadata
todo.initialize("todo-1".into(), "user-1".into(), "Ship it".into());

// Verify
assert_eq!(todo.entity.events()[0].correlation_id(), Some("req-abc-123"));
```

Entity metadata is **transient** — it's not serialized with the entity. It's a request-scoped context you set before each command invocation.

### Propagating Metadata to Outbox Messages

Use `encode_for_entity` to create outbox messages that automatically inherit the entity's metadata context:

```rust
let mut outbox = OutboxMessage::encode_for_entity(
    format!("{}:created", order.entity.id()),
    "OrderCreated",
    &payload,
    &order.entity,  // metadata propagates automatically
)?;

repo.outbox(&mut outbox).commit(&mut order)?;
```

The metadata flows through the full chain:

```text
Entity.set_correlation_id("req-123")
  → #[digest] → EventRecord.metadata
  → encode_for_entity → OutboxMessage.metadata
  → OutboxWorkerThread → bus::Event.metadata
  → subscriber receives event with event.correlation_id() == "req-123"
```

### Reading Metadata

All event types expose the same accessor pattern:

```rust
// On EventRecord (event store)
event_record.correlation_id()  // Option<&str>
event_record.causation_id()    // Option<&str>
event_record.meta("user_id")   // Option<&str>

// On OutboxMessage
message.correlation_id()
message.meta("trace_id")

// On bus::Event (received by subscribers)
event.correlation_id()
event.meta("user_id")
```

## In-Process Event Choreography (requires `emitter` feature)

The `emitter` feature (enabled by default) adds in-process event-driven choreography — queue local events during commands and emit them after commit for reactive workflows within a process.

### With `#[sourced(entity, enqueue)]`

The cleanest approach: add `enqueue` to the `#[sourced]` attribute. Every `#[event]` method automatically both records to the entity stream (for replay) and enqueues for in-process emission:

```rust
use sourced_rust::{sourced, Entity};
use sourced_rust::emitter::EntityEmitter;

#[derive(Default)]
struct OrderSaga {
    entity: Entity,
    #[serde(skip, default)]
    emitter: EntityEmitter,
    order_id: String,
    status: String,
}

#[sourced(entity, enqueue)]
impl OrderSaga {
    #[event("OrderStarted")]
    fn start(&mut self, order_id: String) {
        self.entity.set_id(&order_id);
        self.order_id = order_id;
        self.status = "started".into();
    }

    #[event("StepCompleted", when = self.status == "started")]
    fn complete_step(&mut self) {
        self.status = "completed".into();
    }
}
// Generates: OrderSagaEvent enum + impl Aggregate
// All #[event] methods also enqueue for emission
```

### The `#[enqueue]` Macro (Standalone)

For more granular control, or when using `#[digest]` instead of `#[sourced]`, you can use `#[enqueue]` directly on individual methods:

```rust
#[digest("OrderStarted")]
#[enqueue("OrderStarted")]
fn start(&mut self, order_id: String) { /* ... */ }
```

**Custom emitter field**:

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
todo.entity.set_correlation_id("req-abc");
todo.initialize("todo-1".into(), "user-1".into(), "Buy milk".into());

// Derives id, snapshot payload, and metadata from the aggregate automatically
let mut message = OutboxMessage::domain_event("TodoInitialized", &todo)?;

// Commit both atomically
repo.outbox(&mut message).commit(&mut todo)?;
```

For custom payloads or IDs, use `encode_for_entity` instead:

```rust
let mut message = OutboxMessage::encode_for_entity(
    format!("{}:init", todo.entity.id()),
    "TodoInitialized",
    &custom_payload,
    &todo.entity,
)?;
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
order.entity.set_correlation_id("req-123");
order.create("order-1".into(), "customer-1".into());

let mut outbox = OutboxMessage::encode_for_entity(
    "order-1:created",
    "OrderCreated",
    &OrderCreatedPayload { order_id: "order-1".into() },
    &order.entity,  // metadata propagates automatically
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

## Microservice Framework (`microsvc`)

The `microsvc` module provides a convention-based command handler framework for building microservices. Register command handlers on a `Service<R>`, then expose them over HTTP, bus transports, or direct dispatch.

### Defining a Service

A `Service<R>` is generic over a repository type. Register commands with closures or handler modules:

```rust
use std::sync::Arc;
use sourced_rust::{microsvc, HashMapRepository, AggregateBuilder, Queueable};
use serde_json::json;

let service = Arc::new(
    microsvc::Service::new(HashMapRepository::new().queued())
        .command("counter.create", |ctx| {
            let input = ctx.input::<CreateCounter>()?;
            let counter_repo = ctx.repo().clone().aggregate::<Counter>();
            let mut counter = Counter::default();
            counter.create(input.id.clone());
            counter_repo.commit(&mut counter)?;
            Ok(json!({ "id": input.id }))
        })
        .command("counter.increment", |ctx| {
            let input = ctx.input::<IncrementCounter>()?;
            let counter_repo = ctx.repo().clone().aggregate::<Counter>();
            let mut counter: Counter = counter_repo
                .get(&input.id)?
                .ok_or_else(|| microsvc::HandlerError::NotFound(input.id.clone()))?;
            counter.increment(input.amount);
            counter_repo.commit(&mut counter)?;
            Ok(json!({ "value": counter.value }))
        })
);

// Direct dispatch
let result = service.dispatch(
    "counter.create",
    json!({ "id": "c1" }),
    microsvc::Session::new(),
)?;
```

### Guards

Add input validation with `command_guarded`. The guard runs before the handler — if it returns `false`, the command is rejected:

```rust
let service = microsvc::Service::new(HashMapRepository::new().queued())
    .command_guarded(
        "admin.reset",
        |ctx| ctx.role() == Some("admin"),
        |_ctx| Ok(json!({ "reset": true })),
    );
```

### Handler Convention

For larger services, organize handlers into separate files following a convention. Each handler module exports a `COMMAND` name, a `guard`, and a `handle` function:

```rust
// src/handlers/counter_create.rs
pub const COMMAND: &str = "counter.create";

pub fn guard<R>(ctx: &microsvc::Context<R>) -> bool {
    ctx.has_fields(&["id"])
}

pub fn handle<R: Repository + Clone>(
    ctx: &microsvc::Context<R>,
) -> Result<Value, microsvc::HandlerError> {
    let input = ctx.input::<Input>()?;
    let counter_repo = ctx.repo().clone().aggregate::<Counter>();
    let mut counter = Counter::default();
    counter.create(input.id.clone());
    counter_repo.commit(&mut counter)?;
    Ok(json!({ "id": input.id }))
}
```

Register them with the `register_handlers!` macro:

```rust
let service = sourced_rust::register_handlers!(
    microsvc::Service::new(HashMapRepository::new().queued()),
    handlers::counter_create,
    handlers::counter_increment,
);
```

### HTTP Transport (requires `http` feature)

The `http` feature adds an axum-based HTTP transport. Every registered command becomes a `POST /:command` endpoint. Request headers flow into the `Session`:

```rust
use std::sync::Arc;
use sourced_rust::{microsvc, HashMapRepository};

let service = Arc::new(
    microsvc::Service::new(HashMapRepository::new().queued())
        .command("counter.create", |ctx| { /* ... */ Ok(json!({ "id": "c1" })) })
);

// Get an axum Router to compose with other routes
let app = microsvc::router(service.clone());

// Or serve directly
microsvc::serve(service, "0.0.0.0:3000").await?;
```

Routes:

| Method | Path | Description |
|---|---|---|
| `POST` | `/:command` | Dispatch a command. Body = JSON input, headers = session variables. |
| `GET` | `/health` | Health check: `{ "ok": true, "commands": ["counter.create", ...] }` |

```bash
# Create a counter
curl -X POST http://localhost:3000/counter.create \
  -H 'Content-Type: application/json' \
  -H 'x-hasura-user-id: user-42' \
  -d '{"id": "c1"}'

# Increment it
curl -X POST http://localhost:3000/counter.increment \
  -H 'Content-Type: application/json' \
  -d '{"id": "c1", "amount": 5}'

# Health check
curl http://localhost:3000/health
```

### Bus Transports (requires `bus` feature)

Connect a service to the bus for background command processing. Both transports run in a background thread and return a `TransportHandle` for shutdown and stats.

**Listen (point-to-point)** — each message on the queue is consumed by one listener:

```rust
use std::sync::Arc;
use std::time::Duration;
use sourced_rust::{microsvc, bus::{InMemoryQueue, Sender, Event}, HashMapRepository, Queueable};

let queue = InMemoryQueue::new();
let service = Arc::new(
    microsvc::Service::new(HashMapRepository::new().queued())
        .command("counter.create", |ctx| { /* ... */ Ok(json!({ "id": "c1" })) })
);

let handle = microsvc::listen(service.clone(), "counters", queue.clone(), Duration::from_millis(50));

// Send commands to the queue
queue.send("counters", Event::with_string_payload("cmd-1", "counter.create", r#"{"id":"c1"}"#))?;

// Bus event metadata becomes session variables
let event = Event::with_string_payload("cmd-2", "counter.create", r#"{"id":"c2"}"#)
    .with_metadata("x-hasura-user-id", "user-42");

let stats = handle.stop();
println!("handled: {}, failed: {}", stats.handled, stats.failed);
```

**Subscribe (pub/sub fan-out)** — every subscriber sees every event:

```rust
let subscriber = queue.new_subscriber();
let handle = microsvc::subscribe(service.clone(), subscriber, Duration::from_millis(50));
```

### Combining Transports

A single service can handle commands from multiple transports simultaneously — HTTP, bus, and direct dispatch all share the same handlers and repository:

```rust
let service = Arc::new(
    microsvc::Service::new(HashMapRepository::new().queued())
        .command("counter.create", |ctx| { /* ... */ Ok(json!({})) })
);

// Bus transport in background
let bus_handle = microsvc::listen(service.clone(), "counters", queue, Duration::from_millis(50));

// HTTP transport
microsvc::serve(service.clone(), "0.0.0.0:3000").await?;
```

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

Add `#[derive(Snapshot)]` to your aggregate struct. This generates a `TodoSnapshot` struct, a `fn snapshot()` method, and the full `impl Snapshottable` — no boilerplate needed:

```rust
use sourced_rust::{Entity, Snapshot};

#[derive(Default, Snapshot)]
struct Todo {
    entity: Entity,
    user_id: String,
    task: String,
    completed: bool,
}
```

This generates:
- `TodoSnapshot` struct with `id: String`, `user_id: String`, `task: String`, `completed: bool`
- `impl Todo { fn snapshot(&self) -> TodoSnapshot }` — `id` comes from `entity.id()`
- `impl Snapshottable for Todo` with `create_snapshot()` and `restore_from_snapshot()`

Fields with `#[serde(skip)]` (like `emitter: EntityEmitter`) are automatically excluded from the snapshot.

**Custom ID key** — when the entity ID maps to a domain field like `sku`:

```rust
#[derive(Default, Snapshot)]
#[snapshot(id = "sku")]
struct Inventory {
    entity: Entity,
    sku: String,
    available: u32,
}
// InventorySnapshot has `sku: String` and `available: u32` (no extra `id` field)
// restore_from_snapshot calls entity.set_id(&snapshot.sku)
```

**Custom entity field name** — when your entity field isn't named `entity`:

```rust
#[derive(Default, Snapshot)]
#[snapshot(entity = "my_entity")]
struct Widget {
    my_entity: Entity,
    name: String,
}
```

**Manual implementation** — if you need full control, implement the `Snapshottable` trait directly instead of using the derive:

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

## Event Upcasting / Versioning

Event schemas evolve over time. When you add a field to an event (e.g., `priority` to `Initialized`), old serialized events in storage can't deserialize into the new type — especially with bitcode's rigid binary format. **Upcasters** solve this: pure functions that transform old event payloads into the current format at read time, without modifying stored data.

### Defining an Upcaster

An upcaster is a plain function that converts a payload from one version to the next:

```rust
/// Upcasts Initialized v1 (id, task) → v2 (id, task, priority)
fn upcast_init_v1_v2(payload: &[u8]) -> Vec<u8> {
    let (id, task): (String, String) = bitcode::deserialize(payload).unwrap();
    bitcode::serialize(&(id, task, 0u8)).unwrap()  // default priority = 0
}
```

### Registering Upcasters

With `#[sourced]`, add upcasters directly in the attribute:

```rust
#[derive(Default)]
struct Todo {
    entity: Entity,
    task: String,
    priority: u8,
    completed: bool,
}

#[sourced(entity, upcasters(
    ("Initialized", 1 => 2, upcast_init_v1_v2),
))]
impl Todo {
    #[event("Initialized", version = 2)]
    fn initialize(&mut self, id: String, task: String, priority: u8) {
        self.entity.set_id(&id);
        self.task = task;
        self.priority = priority;
    }

    #[event("Completed", when = !self.completed)]
    fn complete(&mut self) {
        self.completed = true;
    }
}
```

Or with the lower-level `#[digest]` + `aggregate!()`:

```rust
impl Todo {
    #[digest("Initialized", version = 2)]
    fn initialize(&mut self, id: String, task: String, priority: u8) { /* ... */ }
}

aggregate!(Todo, entity {
    "Initialized"(id, task, priority) => initialize,
    "Completed"() => complete(),
} upcasters [
    ("Initialized", 1 => 2, upcast_init_v1_v2),
]);
```

Old events stored as `(id, task)` at v1 get transparently upcasted to `(id, task, 0u8)` at v2 during hydration. New events are created at v2 via the `version = 2` parameter on `#[event]` (or `#[digest]`).

### Chaining Upcasters

Upcasters chain automatically. Each transforms one version to the next (v1->v2->v3):

```rust
fn upcast_init_v1_v2(payload: &[u8]) -> Vec<u8> {
    let (id, task): (String, String) = bitcode::deserialize(payload).unwrap();
    bitcode::serialize(&(id, task, 0u8)).unwrap()
}

fn upcast_init_v2_v3(payload: &[u8]) -> Vec<u8> {
    let (id, task, priority): (String, String, u8) = bitcode::deserialize(payload).unwrap();
    bitcode::serialize(&(id, task, priority, String::new())).unwrap()  // add due_date
}

#[sourced(entity, upcasters(
    ("Initialized", 1 => 2, upcast_init_v1_v2),
    ("Initialized", 2 => 3, upcast_init_v2_v3),
))]
impl Todo {
    #[event("Initialized", version = 3)]
    fn initialize(&mut self, id: String, task: String, priority: u8, due_date: String) { /* ... */ }

    #[event("Completed", when = !self.completed)]
    fn complete(&mut self) { self.completed = true; }
}
```

A v1 event automatically chains through v1->v2->v3. A v2 event only goes through v2->v3. A v3 event passes through unchanged.

### How It Works

- **On hydrate**: Before replaying events, the aggregate's registered upcasters are applied. Each event is checked against the upcaster list by event name and version, and transformed if a match is found.
- **On snapshot hydrate**: Only post-snapshot events are upcasted — the snapshot already contains the current state.
- **No stored data modified**: Upcasters are read-time transformations. The event store is never touched.
- **Zero overhead when unused**: If an aggregate has no upcasters, `hydrate()` takes the fast path with no extra allocation.

### The `EventUpcaster` Struct

Under the hood, each upcaster is a plain struct with a function pointer — no traits, no boxing:

```rust
pub struct EventUpcaster {
    pub event_type: &'static str,
    pub from_version: u64,
    pub to_version: u64,
    pub transform: fn(payload: &[u8]) -> Vec<u8>,
}
```

You can also use `upcast_events()` directly for custom hydration logic:

```rust
use sourced_rust::{upcast_events, EventUpcaster};

let upcasters: &[EventUpcaster] = &[/* ... */];
let upcasted = upcast_events(events, upcasters);
```

## Project Structure

```
src/
  core/       # Entity, events, repository traits, aggregate helpers
  bus/        # Service bus, publishers, subscribers
  emitter/    # In-process event emitter helpers
  hashmap/    # In-memory repository
  lock/       # Lock trait, LockManager trait, InMemoryLock
  microsvc/   # Command handler framework: service, context, session, transports
  queued/     # Queue-based locking wrapper
  read_model/ # Read model store traits and InMemoryReadModelStore
  snapshot/   # Snapshot store traits, InMemorySnapshotStore, SnapshotAggregateRepository
  outbox/     # Outbox message aggregate + worker + publishers
  lib.rs      # Public exports
```

## Running Tests

```bash
cargo test                 # all tests (default features)
cargo test --features http # includes HTTP transport tests
```

## Examples

- `tests/sourced/` - `#[sourced]` macro with typed event enum, TryFrom, and aggregate hydration
- `tests/sourced_upcasting/` - `#[sourced]` with upcasters (v1->v2->v3 chains)
- `tests/sourced_enqueue/` - `#[sourced(entity, enqueue)]` integrated choreography
- `tests/todos/` - Basic entity workflow (using `#[digest]` + `aggregate!()`)
- `tests/snapshots/` - Snapshot creation, loading, and partial replay
- `tests/sourced_snapshot/` - `#[derive(Snapshot)]` with custom ID keys, `serde(skip)` exclusion, and custom entity fields
- `tests/upcasting/` - Event versioning with v1->v2->v3 upcasters, chaining, and snapshot integration
- `tests/sagas/distributed.rs` - Multi-service saga with outbox pattern (fan-out and point-to-point)
- `tests/sagas/orchestration.rs` - Saga orchestration with compensation
- `tests/microsvc/` - Microservice framework: dispatch, session, convention, bus transports, HTTP transport

## License

MIT. See `LICENSE`.
