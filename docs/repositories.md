# Repository Boundary

Distributed, currently published from the `distributed` crate, keeps the
repository API async-only. The repository traits are stream-aware and are used
by the in-memory, SQLite, and Postgres adapters.

Event-store adapters load and commit streams with `StreamIdentity`, the pair
`(aggregate_type, aggregate_id)`, rather than an ID-only key.
`Aggregate::aggregate_type()` provides the type component; the default uses
Rust's type name for development compatibility, but production persistence
should override it with an explicit durable name through `impl_aggregate!(...,
aggregate_type = "...")`, `aggregate!(..., aggregate_type = "..." { ... })`, or
`#[sourced(..., aggregate_type = "...")]`.

## Core Traits

- `GetStream` loads one or more event streams by full identity.
- `TransactionalCommit` commits `CommitBatch` values with stream
  writes, read-model write plans, and snapshots under one backend transaction.
- `ReadModelWritePlanStore` and `RelationalReadModelQueryStore`
  mirror the relational read-model write and primary-key load surfaces for
  repository adapters.
- `SnapshotStore` keys rebuildable snapshot cache records by full stream
  identity. The record envelope carries stream identity, covered event version,
  snapshot payload type/version, payload codec metadata, cache metadata, and
  timestamp.
- `OutboxStore` exposes async claim/update operations for durable outbox
  table stores. Aggregate repositories commit outbox rows transactionally, but
  workers do not hydrate outbox messages through aggregate repositories.

## In-Memory Reference

`InMemoryRepository`, `InMemoryReadModelStore`, and `InMemorySnapshotStore`
implement the repository traits as a behavioral reference for conformance tests.
The in-memory implementation is not a production I/O adapter; it exists so
Postgres, SQLite, and other persistent backends can be tested against the same
stream-aware contract before SQL code lands.

The Postgres repository implements the traits directly with `sqlx`; database I/O
is not hidden behind blocking wrappers in normal async runtimes.

## SQLite Adapter

The optional `sqlite` feature exports `SqliteRepository`, an async-only
SQL-backed adapter for local persistence and conformance work:

```rust
let repo = distributed::SqliteRepository::connect_and_migrate("sqlite::memory:").await?;
```

`SqliteRepository::migrate` applies explicit SQLite migrations from
`migrations/sqlite`. Plain construction from an existing pool does not create
tables implicitly, so applications can control bootstrap order.

The SQLite adapter persists aggregate events, relational read-model write
plans, processed-message marks, and snapshots in one SQL transaction when they
are staged through `CommitBatch`. It intentionally does not claim Postgres
production readiness: Postgres-specific column types, isolation behavior, error
mapping, deployment, and migration validation still belong to the Postgres
adapter and its own tests.

## Postgres Adapter

The optional `postgres` feature exports `PostgresRepository`, an async-only
SQLx adapter for the production SQL event-store path:

```rust
let repo =
    distributed::PostgresRepository::connect_and_migrate(database_url).await?;
```

Local integration tests can use the root `compose.yaml` service:

```bash
docker compose up -d postgres
DATABASE_URL=postgres://sourced:sourced@localhost:5432/distributed \
  cargo test --features postgres --test postgres_repository
```

The SQLite and Postgres adapters persist aggregate event streams, read-model
write plans, processed-message marks, snapshots, and outbox rows through
explicit migrations plus registered table schemas. Relational read-model
mutations (`upsert`, sparse `patch`, and `delete`) are lowered into SQL writes
against the tables generated from `#[derive(ReadModel)]` / `RelationalReadModel`
schema metadata, including JSON/JSONB columns and `_sourced_version` optimistic
versions. SQL repositories do not persist generic document rows; whole-view
state that belongs in SQL should be modeled as a declared read-model table with
an `id` column and JSON/JSONB columns for semistructured fields.
