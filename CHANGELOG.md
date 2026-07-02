### What's changed in v2.3.0

* feat: outbox dedup + perf cleanup (remove OutboxWorker stack, batch settlement) (#106) (by @patrickleet)

  * remove vestigial OutboxWorker publish stack

  Delete the parallel outbox publish pipeline (OutboxWorker, DrainResult,
  ProcessOneResult, OutboxPublisher, LogPublisher, LocalEmitterPublisher):
  a complete second claim -> publish -> settle state machine that mutated
  loaded rows in memory without durable settle, duplicating the production
  drain path in OutboxDispatcher::dispatch_claimed with its own degenerate
  message shape (event_type + raw bytes + metadata map instead of the
  canonical bus Message).

  Its only consumers were the todos integration tests, now ported to
  OutboxDispatcher with a recording MessagePublisher. LocalEmitterPublisher
  had no other users (the emitter feature's real path is src/emitter
  enqueue), so it is not recreated. Pre-release: no deprecation shims.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01DzYSVLas93c7LbgHJWsW7n

  * perf: convert outbox rows to transport messages by value

  The immediate-publish path cloned each staged row into the publish hook,
  then From<&OutboxMessage> for Message cloned the payload bytes and every
  metadata string a second time. Every dispatch path (dispatch_claimed,
  publish hook, outbox source) owns its row by the time it maps, so the
  conversion is now From<OutboxMessage> (by value) and the payload,
  event type, id, and metadata strings move instead of being re-cloned.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01DzYSVLas93c7LbgHJWsW7n

  * refactor: share one publish-then-settle path for dispatcher and hook

  OutboxDispatcher::dispatch_claimed and BusOutboxPublishHook carried the
  same publish -> complete-or-record_failure sequence in two places.
  Extract it as publish_and_settle, used by both. The hook still never
  re-claims: it is handed rows already claimed inside the commit
  transaction, and publish_and_settle only settles claims it is given.

  OutboxPublishHook::publish_claimed now takes the commit's claimed rows
  as one batch (callers looped per row anyway), which also sets up
  batched settlement for the after-commit path.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01DzYSVLas93c7LbgHJWsW7n

  * perf: batch outbox settlement and bound publish concurrency

  dispatch_claimed settled strictly serially: one publish round trip plus
  one complete UPDATE round trip per message.

  - Add OutboxStore::complete_many (default: serial loop). Postgres settles
    the batch in one UPDATE ... FROM unnest(ids, workers, attempts)
    statement with the same per-claim predicate as complete, diagnosing
    unapplied claims to the same NotFound/InvalidState errors; SQLite runs
    the per-claim UPDATEs in one transaction (one commit/fsync per batch;
    an IN-list cannot carry the per-claim worker/attempt predicate); the
    hashmap store settles under a single write lock. Additive only in the
    repo backends: new fns, no restructuring.
  - publish_and_settle now publishes with bounded concurrency
    (buffer_unordered) and settles all successes in one complete_many call;
    publish failures remain settled individually via record_failure.
  - OutboxDispatcher::with_publish_concurrency(NonZeroUsize) exposes the
    publish window. It defaults to 1 because outbox claim order is
    created-at order and consumers may rely on it: 1 preserves strict
    ordering on the wire; higher values overlap publish round trips but may
    deliver out of order. The after-commit hook always uses 1 — a commit's
    rows are one aggregate's events, where relative order matters.
  - If the batched complete fails, published-but-unsettled rows stay in
    flight and are re-published after lease expiry — the same at-least-once
    window a crash between publish and settle always leaves.
  - Adds futures-util (no default features) to the core crate for
    buffer_unordered; the runtime-agnostic core stays executor-free.
  - Conformance: worker_completes_claims_in_one_batch runs complete_many
    against hashmap, Postgres, and SQLite.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01DzYSVLas93c7LbgHJWsW7n

  * refactor: share claim_order_key between outbox store ordering helpers

  sort_by_claim_order and claim_order_ids each restated the claim ordering
  (created-at, then message id). One claim_order_key fn now defines it and
  both helpers sort by it, so the two orderings cannot drift apart.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01DzYSVLas93c7LbgHJWsW7n

  * chore: consolidate outbox_worker test block_on into one test util

  The busy-poll test executor was copy-pasted into three outbox_worker
  test modules (two more copies died with the OutboxWorker stack). One
  #[cfg(test)] testing module now owns it; src/bus copies are untouched.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01DzYSVLas93c7LbgHJWsW7n

  * fix: preserve sub-second precision in outbox lease deadlines

  lease_deadline_secs truncated now + lease to whole seconds (.as_secs()),
  and claim() rebuilt the SystemTime from that u64 — so a sub-second lease
  produced a deadline at or before now (expired at birth), and every lease
  lost up to a second. claim() now takes the deadline as a SystemTime and
  lease_deadline computes now + lease at full precision, keeping the
  overflow and before-epoch validation. The stores already persist
  leased_until with sub-second precision (f64 epoch / nanosecond storage),
  so only the in-memory computation was lossy.

  Unit test covers a sub-second lease: deadline is exactly now + lease and
  is not expired until the lease elapses.

  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01DzYSVLas93c7LbgHJWsW7n

  ---------

  Co-authored-by: Claude Fable 5 <noreply@anthropic.com>


See full diff: [v2.2.4...v2.3.0](https://github.com/hops-ops/distributed/compare/v2.2.4...v2.3.0)
