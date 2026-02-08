# Read Models

Read models are denormalized views derived from event-sourced aggregates.
They give you fast, purpose-built query models shaped for your UI or API
consumers — without polluting your domain aggregates with read concerns.

`sourced_rust` supports three strategies for keeping read models up to date,
each suited to different consistency requirements:

| Strategy | Consistency | Use when… |
|---|---|---|
| Eventually consistent | Stale by ms–seconds | Dashboards, search, reports |
| Atomic commit | Immediate | Command response must include the updated view |
| QueuedReadModelStore | Serialized | Concurrent writers to the same view (rare) |

---

## Defining a Read Model

Derive `ReadModel` on any serializable struct:

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
    pub board: Vec<Vec<Cell>>,
}
```

- `#[readmodel(collection = "...")]` — maps to a table/collection/key-prefix.
  Defaults to the snake_case struct name + `"s"` if omitted.
- `#[readmodel(id)]` — marks the unique identifier field.
  Defaults to a field named `id` if omitted.

Read models are stored and loaded through the `ReadModelStore` trait.
Every repository that implements `ReadModelStore` gets a typed accessor:

```rust
use sourced_rust::ReadModelsExt;

// Typed read model access
let view = repo.read_models::<GameView>().get("game-42")?;
repo.read_models::<GameView>().upsert(&updated_view)?;
```

---

## 1. Eventually Consistent (The Default Path)

This is the bread-and-butter pattern for CQRS: events are published through
an outbox, and a separate denormalizer/projector process consumes them to
update read models.

```
Command → Aggregate → Outbox → [worker] → Denormalizer → Read Model
```

### How it works

1. Commands produce events recorded on the aggregate via `#[digest]`.
2. An `OutboxMessage` is committed alongside the aggregate.
3. An `OutboxWorker` polls for pending messages and publishes them
   through an `OutboxPublisher`.
4. A subscriber (denormalizer) receives the event and updates the
   appropriate read model(s).

```rust
use sourced_rust::{CommitBuilderExt, OutboxMessage};

// After handling a command...
counter.increment(5);

let outbox = OutboxMessage::encode(
    "counter-1:incremented",
    "CounterIncremented",
    &counter.value(),
)?;

repo.outbox(outbox).commit(&mut counter)?;
```

On the consumption side:

```rust
use sourced_rust::{OutboxWorker, LogPublisher};

let mut worker = OutboxWorker::new(LogPublisher::new())
    .with_batch_size(100)
    .with_max_attempts(5);

// In a loop or background task:
let mut messages = repo.claim_outbox_messages("worker-1", 100, lease)?;
let result = worker.process_batch(&mut messages);
```

### When to use it

- Dashboards, analytics, search indexes, reports
- Any view where "a few milliseconds stale" is perfectly fine
- Cross-service views (the outbox guarantees at-least-once delivery)
- Most read models in most systems

This is the **default recommendation**. Start here unless you have a
specific reason not to.

---

## 2. Atomic Commits (The Gaming Pattern)

Sometimes the response to a command must include the fully consistent,
updated view. The canonical example: a single-player game where the
backend processes a move and returns the complete updated game state.

`sourced_rust` supports this with atomic commit builders that write the
aggregate and one or more read models in a single transaction:

```rust
use sourced_rust::CommitBuilderExt;

// Player submits a move
game.make_move(player_move);

// Build the view from the updated aggregate
let mut view = GameView::from(&game);

// Commit aggregate + view atomically
repo.readmodel(&view).commit(&mut game)?;

// Return `view` to the client — it reflects the committed state
```

### Chaining multiple read models and outbox

You can chain any combination of read models and outbox messages:

```rust
repo.readmodel(&game_view)
    .readmodel(&leaderboard_entry)
    .outbox(move_event)
    .commit(&mut game)?;
```

Order doesn't matter — the commit writes everything in one batch.

### Standalone read model writes

If you need to write read models without an aggregate (e.g., materializing
a view in a denormalizer):

```rust
repo.readmodel(&view1)
    .readmodel(&view2)
    .commit_all()?;
```

### When to use it

- Single-player games or turn-based games where the response is the game state
- Wizard/multi-step workflows where each step returns the updated form state
- Any command where the caller **needs** the updated view in the response

### Why you don't need read model locking here

If you're using a `QueuedRepository` (which serializes writes per entity),
the aggregate already has a lock. The read model commit just rides along
inside that lock scope. No additional read model locking is needed.

```
Request A (entity "game-42") ──→ [entity lock] ──→ commit(agg + view) ──→ unlock
Request B (entity "game-42") ──→ [waits]       ──→ [entity lock] ──→ commit ──→ unlock
```

Each request gets the aggregate, processes its command, and atomically
commits the aggregate + view. The entity-level lock serializes them.
The read model is always consistent with the aggregate because they're
written together.

---

## 3. QueuedReadModelStore — The Escape Hatch

`QueuedReadModelStore` wraps any `ReadModelStore` and adds per-instance
locking: `get` acquires a lock, writes (`upsert`, `commit`, etc.) release it.

```rust
use sourced_rust::{QueuedReadModelStore, HashMapRepository, ReadModelsExt};

let store = QueuedReadModelStore::new(HashMapRepository::new());

// get() acquires a lock on "counter-1" in the "counter_views" collection
let loaded = store.read_models::<CounterView>().get("counter-1")?.unwrap();

// Modify the view...
let mut updated = loaded.data;
updated.set_value(42);

// upsert() releases the lock
store.read_models::<CounterView>().upsert(&updated)?;
```

If another thread calls `get` on the same read model instance while it's
locked, it blocks until the lock is released. Different read model types
(different collections) with the same ID do **not** contend.

### Peeking without locking

```rust
use sourced_rust::ReadOpts;

// Read without acquiring a lock
let peeked = store.get_model_with::<CounterView>("counter-1", ReadOpts::no_lock())?;
```

### Aborting

If you decide not to write after reading, release the lock manually:

```rust
store.abort::<CounterView>("counter-1")?;
```

### When you might think you need it

The typical scenario: two different aggregates updating the same read model
concurrently. For example, an `Order` aggregate and an `Inventory` aggregate
both updating a `ProductDashboardView`.

### Why you probably don't

Before reaching for `QueuedReadModelStore`, consider these alternatives:

**Bad aggregate boundaries.** If two aggregates must update the same view
atomically, ask whether they should be one aggregate. The whole point of
aggregate boundaries is to define consistency boundaries. If two things
need transactional consistency, they belong together.

**Use a saga.** If the aggregates are genuinely separate but their effects
need coordination, that's what sagas are for. An `OrderPlaced` event
triggers an inventory reservation through a saga — not through shared
mutable state.

**Eventually consistent is fine.** Most cross-aggregate views don't need
real-time consistency. A dashboard that's 100ms stale is fine. Use the
outbox + denormalizer pattern and let each event update its slice of the
view independently.

### When it's legitimately useful

You've considered the above and still need it. The canonical example:
**seat holds** or **ticket reservations** — where the lock itself is the
feature ("held for you for X minutes"), not a concurrency workaround.

In these cases, the read model represents a resource with contention by
design, and the lock semantics map directly to the domain concept.

---

## Decision Flowchart

```
Do you need the updated view in the command response?
│
├─ No
│  └─ Use eventually consistent (outbox + denormalizer)
│
└─ Yes
   └─ Use atomic commit: repo.readmodel(&view).commit(&mut agg)
      │
      └─ Is there concurrent write contention on the view
         from different aggregates?
         │
         ├─ No
         │  └─ You're done. Entity locking handles it.
         │
         └─ Yes
            ├─ Should the writers be one aggregate?  → Merge them.
            ├─ Should this be a saga?                → Use a saga.
            ├─ Is eventual consistency acceptable?   → Use the outbox.
            └─ Still need it?                        → Use QueuedReadModelStore.
```

---

## API Quick Reference

### ReadModelStore methods (via `read_models::<M>()`)

| Method | Description |
|---|---|
| `get(id)` | Load by ID. Returns `Option<Versioned<M>>` |
| `upsert(model)` | Insert or replace |
| `insert(model)` | Insert; fails if exists |
| `update(model, version)` | Optimistic concurrency update |
| `delete(id)` | Delete by ID |
| `find(predicate)` | Find all matching |
| `find_one(predicate)` | Find first matching |

### CommitBuilder (via `CommitBuilderExt`)

| Method | Description |
|---|---|
| `repo.readmodel(&view)` | Start a builder with a read model |
| `repo.outbox(msg)` | Start a builder with an outbox message |
| `.readmodel(&view)` | Add another read model to the batch |
| `.outbox(msg)` | Add another outbox message to the batch |
| `.commit(&mut agg)` | Write everything + the aggregate |
| `.commit_all()` | Write everything (no aggregate) |

### QueuedReadModelStore extras

| Method | Description |
|---|---|
| `get_model_with(id, ReadOpts::no_lock())` | Peek without locking |
| `find_models_with(pred, ReadOpts::no_lock())` | Find without locking |
| `lock::<M>(id)` | Manually acquire lock |
| `unlock::<M>(id)` / `abort::<M>(id)` | Manually release lock |
