# Async Repository Boundary

Distributed keeps the synchronous in-memory repository API intact and adds a
parallel async persistence boundary for database-backed adapters.

The async traits are stream-aware. Event-store adapters should load and commit
streams with `StreamIdentity`, the pair `(aggregate_type, aggregate_id)`, rather
than an ID-only key. `Aggregate::aggregate_type()` provides the type component;
the default uses Rust's type name for development compatibility, but production
persistence should override it with an explicit durable name through
`impl_aggregate!(..., aggregate_type = "...")`, `aggregate!(..., aggregate_type =
"..." { ... })`, or `#[sourced(..., aggregate_type = "...")]`.

## Core Traits

- `AsyncGetStream` loads one or more event streams by full identity.
- `AsyncTransactionalCommit` commits `AsyncCommitBatch` values with stream
  writes, read-model write plans, and snapshots under one backend transaction.
- `AsyncReadModelStore`, `AsyncReadModelSessionStore`, and
  `AsyncRelationalReadModelQueryStore` mirror the current document and
  relational read-model surfaces for async adapters.
- `AsyncSnapshotStore` keys snapshots by full stream identity.
- `AsyncOutboxRepositoryExt` exposes async worker operations for durable outbox
  implementations.

Async methods use an `_async` suffix where a synchronous method with the same
name already exists. This keeps `HashMapRepository`, `InMemoryReadModelStore`,
and `InMemorySnapshotStore` source-compatible when both sync and async traits
are imported.

## In-Memory Reference

`HashMapRepository`, `InMemoryReadModelStore`, and `InMemorySnapshotStore`
implement the async traits as a behavioral reference for conformance tests. The
in-memory async implementation is not a production I/O adapter; it exists so
Postgres, SQLite, and other persistent backends can be tested against the same
stream-aware contract before SQL code lands.

The Postgres repository should implement the async traits directly with `sqlx`.
It should not hide database I/O behind the synchronous traits with `block_on`,
`block_in_place`, or a blocking wrapper in normal async runtimes.

## SQLite Adapter

The optional `sqlite` feature exports `SqliteRepository`, an async-only
SQL-backed adapter for local persistence and conformance work:

```rust
let repo = sourced_rust::SqliteRepository::connect_and_migrate("sqlite::memory:").await?;
```

`SqliteRepository::migrate` applies explicit SQLite migrations from
`migrations/sqlite`. Plain construction from an existing pool does not create
tables implicitly, so applications can control bootstrap order.

The first SQLite pass persists aggregate events, transactional document read
models, processed-message marks, and snapshots in one SQL transaction when they
are staged through `AsyncCommitBatch`. It intentionally does not claim Postgres
production readiness: Postgres-specific column types, isolation behavior, error
mapping, deployment, and migration validation still belong to the Postgres
adapter and its own tests.
