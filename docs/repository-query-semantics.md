# Repository Query Semantics

`sourced_rust` separates write-side aggregate storage from read-side query
surfaces. Aggregate repositories are append/replay stores. They are optimized
for loading a stream by ID, appending new event records, and replaying an
aggregate. Broad business queries should normally use read models.

## Aggregate Access

Use `get(id)` or `get(&[ids])` for normal aggregate loads. These are point
lookups by stream ID and are the core durable repository contract.

Use `scan(predicate)`, `scan_one(predicate)`, `scan_exists(predicate)`, and
`scan_count(predicate)` when you intentionally want to enumerate hydrated
entity streams and run a Rust predicate in process. This is suitable for:

- tests and examples,
- small in-memory repositories,
- administrative tooling,
- migrations or diagnostics where full enumeration is acceptable.

It is not a SQL query abstraction. A Postgres repository is not required to
translate arbitrary Rust predicates into SQL, nor is it expected to hydrate
every stream for normal production query workloads.

The historical `find`, `find_one`, `exists`, and `count` names remain as
compatibility aliases for scans. New code should use the `Scan` traits when it
depends on full predicate-scan behavior.

## `findAll` Semantics

If an application asks for `findAll` over aggregates, treat that as an explicit
aggregate scan unless a repository-specific indexed query API is documented.
It does not mean "run an efficient database query over arbitrary aggregate
fields."

If the query is part of the product surface, model it as a read model. Read
models are denormalized, query-shaped data that can be backed by tables,
indexes, materialized views, subscriptions, or any other infrastructure suited
to the workload.

## Indexed Store Queries

Durable repositories may expose store-specific indexed queries, but those
indexes must be declared by schema or migration rather than inferred from
arbitrary aggregate fields at runtime. Keep those APIs separate from `Scan` so
callers can tell whether they are using:

- a point lookup by aggregate ID,
- an intentional full stream scan,
- a declared event-store index,
- a read-model query surface.

## Read Models

Read models are the default home for production queries that filter, sort,
paginate, subscribe, or join across aggregate data. They may be maintained
asynchronously from outbox-published messages, so callers should account for
eventual consistency unless the read model is committed atomically with the
aggregate in the same transaction boundary.
