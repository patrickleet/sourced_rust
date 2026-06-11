### What's changed in v1.8.1

* perf: reduce commit/hydrate hot-path round trips and clones (#87) (by @patrickleet)

  * perf: drop event-history clones on the hydrate hot path

  hydrate() previously deep-cloned the entire event history via
  events().to_vec() on every replay even with no upcasters, purely to
  satisfy the borrow checker. Take the events out of the owned entity,
  replay from the local Vec, and restore via load_from_history. The
  no-upcaster path now replays with zero clones; the upcaster path keeps
  its single bounded clone (the durable history is restored verbatim).

  The snapshot tail path (hydrate_prepared_snapshot) did a similar
  .cloned() to collect post-snapshot events. Replay the common
  (no-upcaster) path directly from a filtered borrow so the tail is no
  longer cloned, while still restoring the full history so new_events()
  slicing on the next commit is unchanged. PR #81's snapshot+tail
  fetch behavior is preserved.

  Adds Entity::take_events / Entity::restore_history for the
  borrow-out-then-restore pattern.

  Hydrated state is identical: same replay order, same final
  version/committed_version, same upcaster semantics.

  * perf: batch commit inserts and collapse get_streams N+1

  commit_batch issued one INSERT per event and one INSERT per outbox
  message inside the open transaction, holding row/index locks across
  many round trips. Batch each into a single multi-row INSERT built with
  QueryBuilder::push_values (postgres) — and the same, chunked under
  SQLite's bound-parameter limit (sqlite). The per-stream version
  pre-check is kept: it enforces sequence contiguity, which the unique
  PK alone does not catch for expected<actual gaps.

  Conflict detection is unchanged. An event unique violation still maps
  to ConcurrentWrite (postgres re-reads the conflicting stream's version
  over the pool because a failed statement aborts the tx; sqlite re-reads
  in-tx since SQLite does not abort on a constraint error). An outbox
  unique violation still maps to DuplicateOutboxMessageInBatch.

  get_streams looped awaiting a query per identity (N+1). Replace with one
  WHERE aggregate_type = $1 AND aggregate_id = ANY($2) ORDER BY
  aggregate_id, sequence query per aggregate type (sqlite uses a bound IN
  list, having no array type), splitting the flat result into entities
  client-side. Callers of get_all already accept storage-order results.

  Ordering and hydrated state are unchanged.

  * perf: avoid payload clone on NATS publish

  publish() owns the Message and drops it immediately after, but cloned
  message.payload before handing it to JetStream. Move the buffer out with
  std::mem::take and convert via Bytes::from(Vec<u8>) (zero-copy) instead.

  Behavior is unchanged: the same bytes are published.

  Implements [[tasks/commit-hydrate-hot-path-efficiency]]


See full diff: [v1.8.0...v1.8.1](https://github.com/hops-ops/distributed/compare/v1.8.0...v1.8.1)
