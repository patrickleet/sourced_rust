# Research & Roadmap

Based on a review of the Rust ES ecosystem (cqrs-es, disintegrate, esrs, eventually-rs, kameo_es/SierraDB) and mature frameworks in other languages (Axon, EventFlow, Marten).

---

## Prioritized Work Items

### Core Improvements

#### Async traits
All competing frameworks are async-first. Current traits (`Repository`, `Commit`, `Get`, etc.) are synchronous. This blocks integration with:
- Async databases (sqlx, sea-orm)
- Async message brokers (rdkafka, lapin)
- Async web frameworks (axum handlers)

Approach: Add async versions of the core traits. Could be feature-gated or a parallel set of traits. `tokio` is already a dev dependency.

#### Event upcasting / versioning
Only `cqrs-es` has this in the Rust ecosystem. The idea:
- Add a `version` field to `EventRecord` (or event name becomes `"EventName.v1"`)
- `EventUpcaster` trait: `can_upcast(event_type, version) -> bool` + `upcast(event) -> event`
- Upcasters run automatically during replay/hydration
- Allows old serialized events to be transformed to current schema without data migration

Essential for long-lived systems where event schemas evolve over time.

#### Typed enum events (auto-generated from aggregate macro)
Currently events are string-named with bitcode blobs. The `aggregate!` macro matches on strings. Competing frameworks (cqrs-es, disintegrate) use Rust enums giving compile-time exhaustiveness checking.

Explore whether the `aggregate!` macro (or a new derive) can auto-generate a typed event enum from the aggregate's methods. This would:
- Give compile-time safety when adding/removing events
- Simplify the `aggregate!` macro syntax
- Potentially simplify Entity configuration

### Architecture Patterns

#### Lightweight saga/process manager trait
Current approach: sagas are just aggregates (which works). Explore whether a thin trait adds value:

```rust
// Strawman — just an idea to evaluate
trait ProcessManager: Aggregate {
    type TriggerEvent;
    fn handle_event(&mut self, event: Self::TriggerEvent) -> Vec<Command>;
    fn is_complete(&self) -> bool;
    fn compensate(&mut self) -> Vec<Command>;  // rollback path
}
```

The question: does formalizing this as a trait give enough value over "it's just an aggregate that reacts to events"? Look at the saga tests to see if a trait would reduce boilerplate.

### Later

- **Postgres backend** — proves the trait design, makes the library production-usable. Use sqlx.
- **API docs** — `cargo doc` with doc comments on all public traits/types.
- **Publish to crates.io** — after the above items stabilize.
- **Domain service** — TBD whether to keep. Not documenting further until decided.

---

## Ecosystem Comparison (Reference)

### Feature Matrix

| Feature | sourced_rust | cqrs-es | disintegrate | esrs | eventually-rs |
|---------|-------------|---------|-------------|------|---------------|
| Aggregates | Yes (PORS + macros) | Yes (trait) | Decision pattern | Yes (trait) | Yes (DDD) |
| Event storage | In-memory | PG/MySQL/Dynamo | PG/In-memory | PG only | PG/In-memory |
| Projections/Read models | Yes (3 strategies) | Query trait | EventListener | EventHandler (2 types) | Projection trait |
| Snapshots | Yes (frequency-based) | Yes | Via StateQuery | Yes (Nth event) | No |
| Outbox pattern | Yes (first-class) | No | No | Implicit (TransactionalEventHandler) | No |
| Service bus (pub/sub + P2P) | Yes | No | No | No | No |
| Saga support | Via aggregates | Via handlers | Via EventListener | Via handlers | Via Subscription |
| Event upcasting | **No** | **Yes** | No | No | Partial |
| Async | **No** | Yes | Yes | Yes | Yes |
| Testing framework | N/A (simple code) | Given-When-Then | In-memory store | Manual | AggregateRootBuilder |
| Optimistic concurrency | Yes | Yes | Yes (+ validation queries) | Yes (+ pessimistic) | Yes |
| Pessimistic locking | Yes (QueuedRepo) | No | No | Yes (lock_and_load) | No |
| Event metadata | Yes | Yes | Yes | Yes | Yes |
| Derive macros | digest, aggregate, ReadModel | No | Yes (Event, StateQuery) | No | No |
| Observability | No | No | No | Yes (tracing) | No |
| Pluggable infra (all concerns) | **Yes (6 traits)** | Partial (storage) | Partial (storage) | No (PG only) | Partial (storage) |

### sourced_rust differentiators (things others don't have)
- **Outbox pattern as first-class** with fan-out and point-to-point routing
- **Service bus** with both pub/sub and send/listen patterns
- **Read model atomic commits** with decision flowchart for choosing consistency strategy
- **6 pluggable infrastructure traits** (storage, messaging, read model store, snapshot store, outbox publishing, locking)
- **Guard conditions on events** (`when = !self.completed`)
- **PORS approach** — domain objects are plain structs, not framework types

### Key competitors at a glance

**cqrs-es** (~463 GitHub stars, most mature)
- Best testing framework, event upcasting, 3 storage backends
- Serverless-first design
- Docs: [doc.rust-cqrs.org](https://doc.rust-cqrs.org/)

**disintegrate** (innovative approach)
- Decision pattern instead of aggregates
- Validation queries for fine-grained optimistic concurrency
- Derive macros for events and state queries
- Docs: [docs.rs/disintegrate](https://docs.rs/disintegrate)

**esrs** (production-proven at Prima.it)
- Transactional vs eventually-consistent event handlers
- Built-in tracing/observability
- Pessimistic locking option
- Docs: [docs.rs/esrs](https://docs.rs/esrs)

**eventually-rs** (pre-1.0, DDD-pure)
- Clean DDD abstractions
- Subscription mechanism for reactive processing
- Limited documentation
- Docs: [docs.rs/eventually](https://docs.rs/eventually)

**kameo_es + SierraDB** (successor to thalo, newest)
- Actor-based (kameo) + distributed event store (SierraDB)
- Historical + live subscriptions with guaranteed delivery
- Very new, worth watching
