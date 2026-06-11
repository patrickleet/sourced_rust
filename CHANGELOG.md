### What's changed in v1.7.3

* fix: skip snapshotted I/O on load + gate decode on schema version (#81) (by @patrickleet)

  * perf: skip snapshotted I/O on load; fix: gate snapshot decode on schema version

  Snapshot hydration previously fetched the ENTIRE event stream and only then
  filtered out events already covered by the snapshot, so a fresh snapshot over a
  long stream still paid full I/O and decode cost — snapshots saved replay CPU but
  not the dominant I/O. And the schema-version field was written but never checked:
  bitcode is positional, so a layout-compatible change to a Snapshot struct would
  decode SUCCESSFULLY into wrong state and then commit new events atop corruption.

  Task 1 — skip already-snapshotted events (perf):
  - Add GetStream::get_stream_tail(identity, after_version) (default delegates to
    get_stream; Postgres/SQLite override with `WHERE sequence > $version`; Queued
    forwards under its lock). The single-aggregate `get` hot path now reads the
    snapshot FIRST, then fetches only the tail.
  - Entity tracks a transient `prefix_version` (events folded into a snapshot and
    not held in memory) so version/committed_version/new_events and event
    sequencing stay correct when only a tail is loaded. Add
    Entity::load_tail_from_history.
  - Remove the O(n) entity_stream_version max-scan; entity.version() is the true
    stream version in both load shapes.

  Task 2 — schema-version gate (fix):
  - Add Snapshottable::SNAPSHOT_VERSION (default 1) and #[snapshot(version = N)] in
    the derive (rejects zero / non-integer with helpful errors; emits the const
    only when set so existing impls are unaffected).
  - Write A::SNAPSHOT_VERSION into each record; on load a mismatch is a CACHE MISS
    -> full replay, never a decode of incompatible bytes.
  - Unify semantics: the public hydrate_from_snapshot now degrades gracefully
    (cache miss -> replay) like the internal path, instead of turning cache misses
    into hard Replay errors.
  - snapshot_type (std::any::type_name) is documented diagnostic-only and is NOT
    gated on (type_name is unstable across compilers; gating would cause spurious
    misses). Schema compatibility is enforced solely by SNAPSHOT_VERSION.

  Tests: sqlite-backed proofs that a snapshot-hydrated load reads only the tail
  (pre-snapshot rows deleted, load still correct), that a stale-schema snapshot
  falls back to replay with correct state, and that snapshot-hydrated state equals
  full-replay state; plus entity tail-sequencing unit tests and a derive
  version-attribute test.

  Implements [[tasks/snapshot-hydration-skip-replayed-events]] and [[tasks/snapshot-schema-version-gating]]

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>

  * refactor(snapshot): drop snapshot_type from persisted record

  snapshot_type was a write-only `type_name` field: written into every
  SnapshotRecord and persisted, but never read to make a decision. Since
  `type_name` is explicitly unstable across compiler versions, it could
  never have been a safe compatibility gate anyway.

  SNAPSHOT_VERSION (Snapshottable::SNAPSHOT_VERSION, written into each
  record and compared on load) is the sole compatibility gate for cached
  snapshots: a version mismatch is treated as a cache miss and the
  aggregate is rebuilt by full replay. With that in place, snapshot_type
  is pure vestigial scaffolding.

  This removes it end to end: the SnapshotRecord field, the constructor
  parameter, the empty-string validation, the now-unused
  DEFAULT_SNAPSHOT_VERSION const, the snapshot_type_name helper, the
  sqlite/postgres SELECT/INSERT/upsert/bind/row-read paths, the migration
  columns and their CHECK constraints, and all test call sites and
  assertions. The SNAPSHOT_VERSION doc comments are reworded to state the
  bump-on-layout-change intent positively rather than framing the default
  as backwards-compat for existing implementations.

  Pre-release cleanup: there is no released version or persisted data to
  stay compatible with.

  Refines [[tasks/snapshot-schema-version-gating]]

  ---------

  Co-authored-by: Claude Fable 5 <noreply@anthropic.com>


See full diff: [v1.7.2...v1.7.3](https://github.com/hops-ops/distributed/compare/v1.7.2...v1.7.3)
