# Sourced Rust

Sourced Rust is a small event-sourcing toolkit for Rust. It keeps your domain model as a plain struct (PORO), inspired by POCO/POJO, while giving you append-only events, replay, and persistence.

## Project Inspiration

Sourced Rust is inspired by the original sourced project by Matt Walters. Patrick Lee Scott, a contributor and maintainer of the original JavaScript/TypeScript version, brought these concepts to Rust and refactored them for the Rust ecosystem.

## Design Goals

- Keep domain objects simple and explicit (Plain Old Rust Structs).
- Make events the source of truth for state.
- Make replay predictable and safe.
- Keep storage pluggable and testable.
- Add optional queue-based locking for serialized workflows.

## Core Concepts

- **Entity**: Holds the event history and a lightweight event emitter. You embed it in your domain structs.
- **EventRecord**: An immutable event with name, args, sequence, and timestamp.
- **Repository**: Persists and loads entities by event history.
- **HashMapRepository**: In-memory repository for tests and examples.
- **QueuedRepository**: Wraps any repository and adds per-entity queue locking (get locks until commit or abort for that ID only).
- **OutboxRecord**: A persisted domain event intended for external publication.
- **OutboxRepository**: Access to pending outbox records (peek or drain).
- **Event Emitter**: Provided by event-emitter-rs and used for local, in-process notifications.

## How It Works

### Write Path (Digest + Commit)

```mermaid
sequenceDiagram
    participant Domain
    participant Entity
    participant Repo
    Domain->>Entity: digest(event)
    Entity->>Entity: append EventRecord
    Domain->>Entity: enqueue(local event)
    Domain->>Repo: commit(entity)
    Repo->>Repo: persist events
    Repo->>Entity: emit_queued_events()
```

### Read Path (Get + Replay)

```mermaid
flowchart LR
    Store[(Event Store)] -->|get| Entity
    Entity -->|events| Replayer[Apply events to domain]
    Replayer --> State[Domain state]
```

### Enqueue (Post-Commit Local Events)

`enqueue` is a lightweight way to schedule in-process events that should run **only after a successful commit**. This keeps side effects out of your domain logic without turning them into durable domain events.

Typical uses:

- Trigger in-memory projections/read models.
- Fire local listeners for integrations.
- Update process-local caches.

The flow is:

- `digest(...)` records the event to the event log.
- `enqueue(...)` registers a local event to emit after commit.
- `commit(...)` persists events and then calls `emit_queued_events()`.

### Outbox (Durable Domain Events)

`outbox` records a **domain event meant for other services**. The repository persists it alongside the event stream, and a separate worker publishes it to your message bus. This keeps local event-sourcing events distinct from cross-service integration events.

Repositories that support outbox are expected to persist the event stream and outbox in the **same transaction**. The in-memory repository does this for tests; SQL backends should do the same to preserve the outbox guarantees.

```mermaid
sequenceDiagram
    participant Domain
    participant Entity
    participant Repo
    participant Outbox as Outbox Worker
    participant Bus as Message Bus
    Domain->>Entity: outbox(event_type, payload)
    Domain->>Repo: commit(entity)
    Repo->>Repo: persist events + outbox
    Outbox->>Repo: claim_outbox(worker, batch, lease)
    Outbox->>Bus: publish
    Outbox->>Repo: complete_outbox(ids)
    Note right of Outbox: on failure -> release_outbox or fail_outbox
```

### Queued Repository Locking (Per Entity)

```mermaid
sequenceDiagram
    participant A as Worker A
    participant Q as QueuedRepository
    participant B as Worker B
    A->>Q: get(id) (locks)
    B->>Q: get(id)
    Note right of Q: B waits only for that entity ID
    A->>Q: commit(entity) / abort(id)
    Q-->>B: get(id) resumes
```

### Microservice Concurrency Pattern

This design fits a common microservice shape: many requests arrive in parallel, but updates to the same entity must be serialized. The queue lock ensures in-order processing per ID while allowing unrelated IDs to proceed concurrently.

```mermaid
flowchart LR
    Requests[Incoming Requests] --> Router{Entity ID}
    Router -->|id: A| QueueA[Queue A]
    Router -->|id: B| QueueB[Queue B]
    Router -->|id: C| QueueC[Queue C]
    QueueA --> WorkerA[Worker]
    QueueB --> WorkerB[Worker]
    QueueC --> WorkerC[Worker]
```

## Usage

### Minimal Domain Model

```rust
use sourced_rust::{Entity, EventRecord, HashMapRepository, Repository};

#[derive(Default)]
struct Todo {
    entity: Entity,
    user_id: String,
    task: String,
    completed: bool,
}

impl Todo {
    fn initialize(&mut self, id: String, user_id: String, task: String) {
        self.entity.set_id(id.clone());
        self.user_id = user_id;
        self.task = task;
        self.completed = false;

        self.entity.digest(
            "Initialize",
            vec![id, self.user_id.clone(), self.task.clone()],
        );
    }

    fn apply(&mut self, event: &EventRecord) -> Result<(), String> {
        match event.event_name.as_str() {
            "Initialize" if event.args.len() == 3 => {
                self.entity.set_id(event.args[0].clone());
                self.user_id = event.args[1].clone();
                self.task = event.args[2].clone();
                self.completed = false;
                Ok(())
            }
            "Initialize" => Err("Invalid Initialize args".to_string()),
            _ => Err(format!("Unknown event: {}", event.event_name)),
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let repo = HashMapRepository::new();

    let mut todo = Todo::default();
    todo.initialize("todo-1".to_string(), "user-1".to_string(), "Buy milk".to_string());

    repo.commit(&mut todo.entity)?;

    let entity = repo.get("todo-1")?.expect("todo missing");
    let mut rebuilt = Todo::default();
    rebuilt.entity = entity;

    // Explicit replay keeps your domain model plain.
    let events = rebuilt.entity.events().to_vec();
    rebuilt.entity.set_replaying(true);
    for event in &events {
        rebuilt.apply(event)?;
    }
    rebuilt.entity.set_replaying(false);

    Ok(())
}
```

### Queued Repository (Serialized Workflows)

```rust
use sourced_rust::{HashMapRepository, Outboxable, Queueable, Repository};

let repo = HashMapRepository::new().queued().with_outbox(); // per-entity queue locks + outbox

let mut entity = repo.get("todo-1")?.expect("todo missing");
// ... mutate entity ...
repo.commit(&mut entity)?; // unlocks

// Or release without changes:
repo.abort("todo-1")?;

// Read-only access without taking the queue lock:
let _ = repo.peek("todo-1")?;
```

### Aggregate Helpers (Low Boilerplate)

You can keep domain code tiny and let the library handle hydration:

```rust
use sourced_rust::{aggregate, Entity, EventRecord};

#[derive(Default)]
pub struct Todo {
    pub entity: Entity,
    // domain fields...
}

enum TodoEvent {
    Initialize { id: String, user_id: String, task: String },
    Complete { id: String },
}

impl TodoEvent {
    fn apply(self, todo: &mut Todo) {
        match self {
            TodoEvent::Initialize { id, user_id, task } => todo.initialize(id, user_id, task),
            TodoEvent::Complete { id: _ } => todo.complete(),
        }
    }
}

impl Todo {
    pub fn replay_event(&mut self, event: &EventRecord) -> Result<(), String> {
        TodoEvent::try_from(event)?.apply(self);
        Ok(())
    }
}

aggregate!(Todo, entity, replay_event, TodoEvent, {
    "Initialize" => (id, user_id, task) => Initialize,
    "Complete" => (id) => Complete,
});
```

Repository ergonomics:

```rust
use sourced_rust::{AggregateBuilder, HashMapRepository, Outboxable, Queueable, RepositoryExt};

let repo = HashMapRepository::new().queued().with_outbox();
let todo = repo.get_aggregate::<Todo>("todo-1")?;

let repo = HashMapRepository::new().queued().with_outbox().aggregate::<Todo>();
let todo = repo.get("todo-1")?;
```

### Outbox (In-Memory Example)

```rust
use sourced_rust::{Entity, HashMapRepository, OutboxRepository, Outboxable, Repository};
use std::time::Duration;

let repo = HashMapRepository::new().with_outbox();
let mut entity = Entity::with_id("todo-1");

entity.outbox("TodoInitialized", "{\"id\":\"todo-1\"}".to_string());
repo.commit(&mut entity)?;

let batch = repo.claim_outbox("worker-1", 100, Duration::from_secs(30))?;
for record in &batch {
    // publish to your message bus
    let _ = (record.event_type, record.payload);
}
let ids: Vec<u64> = batch.iter().map(|record| record.id).collect();
repo.complete_outbox(&ids)?;
```

### Outbox Strategies

Below are two simple ways to consume the outbox. Both use `claim_outbox(...)` and `complete_outbox(...)` to keep the pipeline explicit.

#### 1) Log and Complete

```rust
use sourced_rust::{HashMapRepository, OutboxRepository, Outboxable};
use std::time::Duration;

let repo = HashMapRepository::new().with_outbox();
let batch = repo.claim_outbox("logger-1", 100, Duration::from_secs(30))?;
for record in &batch {
    println!(
        "[OUTBOX] {} v{} {} {}",
        record.aggregate_id,
        record.aggregate_version,
        record.event_type,
        record.payload
    );
}
let ids: Vec<u64> = batch.iter().map(|record| record.id).collect();
repo.complete_outbox(&ids)?;
```

#### 2) Local Event Emitter

```rust
use sourced_rust::{EventEmitter, HashMapRepository, OutboxRepository, Outboxable};
use std::time::Duration;

let repo = HashMapRepository::new().with_outbox();
let mut emitter = EventEmitter::new();

emitter.on("TodoInitialized", |payload| {
    println!("local handler: {}", payload);
});

let batch = repo.claim_outbox("emitter-1", 100, Duration::from_secs(30))?;
for record in &batch {
    emitter.emit(&record.event_type, record.payload.clone());
}
let ids: Vec<u64> = batch.iter().map(|record| record.id).collect();
repo.complete_outbox(&ids)?;
```

#### 3) Outbox Worker + Publisher

```rust
use sourced_rust::{HashMapRepository, LogPublisher, OutboxWorker, Outboxable};
use std::time::Duration;

let repo = HashMapRepository::new().with_outbox();
let mut worker = OutboxWorker::new(repo, LogPublisher::new());

worker.drain_once("logger-1", 100, Duration::from_secs(30), 3)?;

// Or run a loop with exponential backoff when idle:
// worker.drain_loop("logger-1", 100, Duration::from_secs(30), 3, Duration::from_secs(1), Duration::from_secs(30), || false)?;
```

#### Helpers: attempt_outbox / deliver_outbox

If you want built-in retry handling, the outbox module provides two helpers:

- `attempt_outbox(...)` — claim + handle + release on error.
- `deliver_outbox(...)` — same as attempt, but fails after `max_attempts`.

```rust
use sourced_rust::{HashMapRepository, OutboxDelivery, Outboxable};
use std::time::Duration;

let repo = HashMapRepository::new().with_outbox();

let result = repo.deliver_outbox("worker-1", 100, Duration::from_secs(30), 3, |record| {
    // publish to your bus
    let _ = &record.payload;
    Ok::<_, std::io::Error>(())
})?;

assert!(result.claimed >= result.completed);
```

## Patterns for Simple Models (PORO)

Sourced Rust keeps your domain models plain:

- Your domain struct owns an `Entity`.
- You call `digest(...)` when the domain decides something happened.
- You `get(...)` from the repository and explicitly replay events into the domain state.
- Locking and concurrency live in the repository, not your model.

That is the same spirit as POCO/POJO: keep models simple, put infrastructure at the edges.

## Project Structure

```
src/
  entity.rs            # Entity implementation
  event.rs             # Event trait
  event_record.rs      # EventRecord definition
  local_event.rs       # Local event queue item
  outbox_record.rs     # Outbox record definition
  outbox_repository.rs # Outbox access trait
  repository.rs        # Repository trait
  hashmap/repository.rs# In-memory repository
  queued_repository.rs # Queue-based locking wrapper
  lock.rs              # Internal lock primitive
  error.rs             # Repository error types
```

## Running Tests

```
cargo test
```

## Examples

See `tests/todos.rs` and `tests/support/` for a full workflow example with replay, commit_all, and queued locking.

## Roadmap

- Snapshotting for faster entity rebuilding
- Event versioning and upcasting
- Storage backends for common databases
- More domain examples and patterns

## License

MIT. See `LICENSE`.
