# Postgres Event Store Contract

This is the storage contract for the future Postgres repository. It defines the
durable representation and repository behavior before the SQL implementation
lands.

The event table stores aggregate-sourced replay records: write-side history for
rehydrating one aggregate stream. It is not the domain-event bus. Published
domain events, integration events, commands, and transport messages belong in
the outbox/message tables with their own delivery, retry, and idempotency
semantics.

## Repository Scope

The Postgres event store is responsible for:

- loading aggregate streams by identity,
- appending new aggregate event records,
- enforcing optimistic concurrency,
- preserving event payload compatibility metadata,
- participating in one transaction boundary for structured commit batches.

It is not responsible for broad product queries. Aggregate-wide predicate
filtering is a scan/admin concern; production queries should use read models or
declared store-specific indexes.

## Stream Identity

Stream identity is the pair `(aggregate_type, aggregate_id)`.

`aggregate_id` is the entity ID already tracked by `Entity`.

`aggregate_type` must be a stable application-level name, not an accidental Rust
module path. The Postgres implementation should add an explicit aggregate type
source before it writes rows, for example:

- an associated constant or method on the aggregate type,
- a derive/macro attribute such as `#[aggregate(type = "todos")]`,
- a conservative default derived from the Rust type name only for tests.

Do not use `std::any::type_name::<A>()` as the persisted default for production:
module paths can change during refactors and would split or orphan existing
streams.

## Event Table

Recommended table name: `aggregate_events`.

| Column | Type | Notes |
| --- | --- | --- |
| `aggregate_type` | `text` | Stable aggregate type name; `NOT NULL`. |
| `aggregate_id` | `text` | Aggregate stream identifier; `NOT NULL`. |
| `sequence` | `bigint` | One-based stream position; `NOT NULL`. |
| `event_name` | `text` | Stable replay event record name; `NOT NULL`. |
| `event_version` | `integer` | Payload schema version; `NOT NULL DEFAULT 1`. |
| `payload` | `bytea` | Encoded event payload bytes; `NOT NULL`. |
| `payload_codec` | `text` | Codec label, initially `bitcode`; `NOT NULL`. |
| `payload_codec_version` | `integer` | Codec metadata, initially `1`; `NOT NULL`. |
| `metadata` | `jsonb` | Event metadata; `NOT NULL DEFAULT '{}'`. |
| `recorded_at` | `timestamptz` | UTC instant for the event record; `NOT NULL`. |

The DDL must declare `aggregate_type` and `aggregate_id` as `NOT NULL`; the
checks below also reject empty strings. `sequence`, `event_name`,
`event_version`, `payload`, `payload_codec`, `payload_codec_version`,
`metadata`, and `recorded_at` must also be `NOT NULL`, and `metadata` must
default to an empty JSON object.

Required constraints:

```sql
PRIMARY KEY (aggregate_type, aggregate_id, sequence);
CHECK (aggregate_type <> '');
CHECK (aggregate_id <> '');
CHECK (sequence > 0);
CHECK (event_version > 0);
CHECK (payload_codec <> '');
CHECK (payload_codec_version > 0);
```

Required or recommended indexes:

```sql
-- Primary replay path; covered by the primary key but listed as the core access pattern.
(aggregate_type, aggregate_id, sequence);

-- Diagnostics, migrations, and upcaster audits.
(aggregate_type, event_name, event_version);

-- Optional operational/audit browsing.
(recorded_at);
```

The implementation may add a generated `stream_id` for internal convenience,
but it must not replace the public identity contract. If present, `stream_id`
must be derived from `(aggregate_type, aggregate_id)` and must remain unique
with those columns.

## Codec Validation and Error Model

Payload codec metadata is part of the durable compatibility contract. The
initial supported codec tuple is:

```text
payload_codec = "bitcode"
payload_codec_version = 1
```

`event_version` describes the domain event payload schema and drives aggregate
upcasters. `payload_codec` and `payload_codec_version` describe how bytes are
encoded. These fields must be validated independently.

On write, the repository must reject event, snapshot, read-model, and outbox
payload rows with an unknown codec label, an unsupported codec version, or empty
codec metadata before inserting any row in the transaction. The initial error
should be a `RepositoryError::Model` wrapping the existing
`EventRecordError::unsupported_codec(...)` message shape.

On read, row decoding must reject unknown codec labels or versions unless the
repository has an explicit decoder or migration path for that codec tuple.
Aggregate payload decode failures during replay should surface as
`RepositoryError::Replay`; row-level codec metadata failures before replay
should surface as `RepositoryError::Model`.

Postgres tests should cover:

- insert/write rejection for unsupported `payload_codec`;
- insert/write rejection for unsupported `payload_codec_version`;
- read rejection for a row containing an unknown codec tuple;
- successful `bitcode` version `1` event and snapshot round trip;
- upcaster behavior remaining keyed by `event_version`, not by codec version.

## Snapshot Table

Snapshots should live in a side table, not as mixed rows in `aggregate_events`.
That keeps the event table append-only and avoids a `snapshot` discriminator in
the event log.

Recommended table name: `aggregate_snapshots`.

| Column | Type | Notes |
| --- | --- | --- |
| `aggregate_type` | `text` | Same stable aggregate type as events; `NOT NULL`. |
| `aggregate_id` | `text` | Same aggregate ID as events; `NOT NULL`. |
| `version` | `bigint` | Stream sequence covered by this snapshot; `NOT NULL`. |
| `snapshot_type` | `text` | State snapshot payload type; `NOT NULL`. |
| `snapshot_version` | `integer` | State snapshot payload version; `NOT NULL`. |
| `payload` | `bytea` | Encoded state snapshot payload bytes; `NOT NULL`. |
| `payload_codec` | `text` | Codec label; `NOT NULL`. |
| `payload_codec_version` | `integer` | Codec metadata; `NOT NULL`. |
| `metadata` | `jsonb` | Cache metadata; `NOT NULL`, default `{}`. |
| `recorded_at` | `timestamptz` | UTC instant for the snapshot; `NOT NULL`. |

The DDL must declare `aggregate_type` and `aggregate_id` as `NOT NULL`; the
checks below also reject empty strings. `version`, `snapshot_type`,
`snapshot_version`, `payload`, `payload_codec`, `payload_codec_version`,
`metadata`, and `recorded_at` must also be `NOT NULL`.

Required constraints and indexes:

```sql
PRIMARY KEY (aggregate_type, aggregate_id);
CHECK (aggregate_type <> '');
CHECK (aggregate_id <> '');
CHECK (version > 0);
CHECK (snapshot_type <> '');
CHECK (snapshot_version > 0);
CHECK (payload_codec <> '');
CHECK (payload_codec_version > 0);
```

The first implementation is latest-only: writing a snapshot cache record
upserts the `(aggregate_type, aggregate_id)` row. Hydration should load that
record, then replay event rows where `sequence > snapshot.version` ordered
ascending. If no usable snapshot exists, hydrate from sequence `1`.

If the newest snapshot version exceeds the current maximum event sequence for
the stream, the implementation should ignore that cache record and hydrate from
sequence `1`. Snapshot cache fallback should be observable when tracing exists,
but it should not turn a recoverable cache miss into command failure.

Snapshot retention is implementation-specific but must be explicit. The current
SQL adapters retain only the latest cache record per stream. Future adapters may
retain last `N` or time-based cache records, but they must never prune aggregate
events.

## Transactional Read Models

Postgres read-model write plans write relational table rows inside the same
repository transaction as aggregate events. Mutations are written to the
registered read-model tables generated from schema metadata
(`bootstrap_table_schema_for_dev` for tests/local development, migration
artifacts for managed environments). Those writes use the model's declared
columns directly, including `jsonb` columns for collection fields and
`_sourced_version` for optimistic row versions.

There is no generic SQL document table in this repository contract. If a
command-side view needs whole-view state in SQL, define a read-model table with
an `id` column and one or more `jsonb` columns for the semistructured data.
Generic document mutations require a dedicated document adapter rather than the
Postgres event-store repository.

`read_model_processed_messages` stores idempotency marks for distributed
projectors that commit a read-model write plan and mark a message processed in
one transaction:

```sql
PRIMARY KEY (consumer_name, message_id);
CHECK (consumer_name <> '');
CHECK (message_id <> '');
```

`read_model_processed_messages` is shared by relational write-plan commits so
projectors can atomically write rows and record consumed messages.

Relationship include loading is a separate query concern; the transactional
write path persists the row mutations staged by `ReadModelWritePlan`.

## Commit Semantics

`Commit::commit` and `TransactionalCommit::commit_batch` establish the behavior
that the Postgres implementation must preserve.

Before writing:

- normalize each entity into `(aggregate_type, aggregate_id)`;
- reject duplicate stream identities inside one batch before any SQL write;
- compute new event sequences from `entity.committed_version() + 1`;
- reject empty aggregate IDs or aggregate types.

Inside one database transaction:

- verify each stream's stored version matches `entity.committed_version()`;
- insert all new event rows with their computed sequence numbers;
- write transaction-compatible outbox, read model, and snapshot rows included in
  the structured batch;
- commit once only after every write succeeds.

After commit succeeds:

- mark in-memory entities committed;
- update snapshot version metadata;
- return success.

If any validation or write fails:

- no event rows from the batch are visible;
- no read model, outbox, or snapshot rows from the batch are visible;
- entities keep their pending `new_events()` and committed versions for retry.

The database must enforce the sequence invariant with
`PRIMARY KEY (aggregate_type, aggregate_id, sequence)`. A unique violation on
that key maps to `RepositoryError::ConcurrentWrite` after querying the current
stream version for the actual value.

## Optimistic Concurrency

For each stream, the expected stored version is `entity.committed_version()`.
The appended events receive sequences:

```text
expected_version + 1
expected_version + 2
...
```

Two writers appending to the same stream with the same expected version cannot
both commit. Postgres may detect the conflict through an explicit version check,
through the primary-key insert, or both. The public error should identify:

- aggregate type,
- aggregate ID,
- expected version,
- actual stored version.

The current `RepositoryError::ConcurrentWrite { id, expected, actual }` can
represent this initially by formatting `id` as a compact JSON object such as
`{"aggregate_type":"todo","aggregate_id":"todo:1"}`. Generate that string with
the JSON serializer rather than manual concatenation so embedded separators in
either component cannot collide. A later API can split type and ID if needed.

## Locking Model

The Postgres repository enforces optimistic concurrency with the
`(aggregate_type, aggregate_id, sequence)` primary key. That uniqueness
constraint is the authoritative cross-process write-conflict boundary and holds
regardless of any lock — concurrent writers to the same stream collide on the
sequence and one fails.

For workflows that want to *serialize* per-aggregate read/modify/write (rather
than let a stale writer fail and retry), `QueuedRepository` can be backed by a
durable lock manager instead of the default process-local
`InMemoryLockManager`. `PostgresLockManager` (and `SqliteLockManager`)
implement `LockManager` over an `aggregate_locks` lease table:

- a held key is a row carrying an `owner_token` and an `expires_at` computed from
  the database clock (one authoritative clock, so no cross-process skew);
- acquisition is a single atomic conditional upsert (insert when absent, steal
  when expired, re-acquire your own token); contention polls on a configurable
  interval until won or `max_wait` elapses;
- release is scoped to the owner token, so it never frees a holder that
  legitimately reclaimed an expired lease.

This is a **mutual-exclusion optimization, not a fencing guarantee.** A critical
section that outlives `lease_ttl` can be stolen while the original holder still
believes it holds the lock — safe only because the sequence primary key above is
the real boundary (a stale writer fails its optimistic commit rather than
corrupting data). v1 has **no lease renewal**, so set `lease_ttl` above the
worst-case critical section. Rows from crashed holders are reused on the next
acquire of the same key, or swept with `sweep_expired`.

`QueuedRepository` over the in-memory manager remains a process-local convenience
for examples, tests, and single-process adapters.

## Backward Compatibility

Rows or imported JSON records without event metadata deserialize with empty
metadata. Postgres migrations should still write
`metadata jsonb NOT NULL DEFAULT '{}'` so newly stored rows are explicit.

Rows without payload codec metadata should be interpreted only by an explicit
legacy import path. Newly written Postgres rows must always populate
`payload_codec` and `payload_codec_version`.

## Runtime Posture

Postgres access is async-first through the public repository, read-model,
snapshot, inbox, lock, and outbox traits. Do not add production Postgres access
behind blocking adapters or synchronous repository shims; SQL I/O belongs in
async `sqlx` paths.

## TypeORM Lineage

The old `sourced-repo-typeorm@3.2.14` shape is useful history, not the Rust
contract.

Carried forward:

| TypeORM field | Rust/Postgres contract |
| --- | --- |
| `entityType` | `aggregate_type` |
| `id` | `aggregate_id` |
| `method` | `event_name` |
| `version` | `sequence` |
| `data jsonb` | `payload` plus explicit codec metadata |
| `timestamp bigint` | `recorded_at timestamptz` |
| `snapshotVersion` | snapshot row `version` |

Intentionally changed:

- The primary key is `(aggregate_type, aggregate_id, sequence)`, not
  `(entityType, id, method, version)`.
- `method`/`event_name` is not part of stream sequence uniqueness.
- Snapshots move to `aggregate_snapshots` instead of sharing the event table
  through a `snapshot` boolean.
- TypeORM `synchronize: true` is not a migration strategy. Use explicit SQL
  migrations.
- Dynamic aggregate-field indexes are not inherited. Business queries belong in
  read models or declared schema-backed indexes.
- `findAll` is not inherited as an unspecified operation. If used over
  aggregates, it must mean an explicit scan or a documented indexed query.

## Existing Behavioral Tests

The in-memory repository already pins the behaviors the Postgres repository must
match:

- `src/in_memory_repo/repository.rs::duplicate_stream_ids_rejected_before_write`
  verifies duplicate stream IDs are rejected before any write.
- `tests/event_store/main.rs::concurrent_writes_detected` verifies optimistic
  conflicts return `ConcurrentWrite`.
- `tests/event_store/main.rs::partial_conflict_rolls_back_entire_commit`
  verifies a failed mixed-stream append leaves other streams unchanged.

Postgres-specific tests should add:

- schema migration smoke test for all tables, constraints, and indexes;
- successful append and replay of one stream;
- multi-stream `commit_batch` all-or-none behavior;
- duplicate `(aggregate_type, aggregate_id)` in one batch rejected before SQL
  writes;
- concurrent append conflict on the same next sequence;
- snapshot save and load from latest snapshot plus tail events;
- codec rejection for unknown `payload_codec` or `payload_codec_version`;
- read model/outbox/snapshot rollback when included in a transaction-compatible
  batch.

## Out Of Scope

- Implementing the actual Postgres repository.
- Designing the final read-model table layout.
- Designing the outbox message table in detail.
- Changing the established async repository trait surface.
