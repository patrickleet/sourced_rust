### What's changed in v1.7.5

* chore: add red-team concurrency and failure-injection conformance tests (#85) (by @patrickleet)

  Pin guarantees the publish-on-commit + snapshot work introduced that were
  previously only covered by unit tests against fakes. All tests exercise
  EXISTING behavior; none required a production change, and all pass against
  current code (no #[ignore]d tests).

  Tests added:
  - racing_commits_one_wins_one_conflicts (scenario.rs, all 3 backends):
    N tasks load the same aggregate at v0 and commit behind a barrier; asserts
    exactly 1 success, N-1 ConcurrentWrite, and a gap/duplicate-free final
    stream — the unique-PK backstop behind the version-check TOCTOU window under
    real concurrency.
  - expired_outbox_lease_is_reclaimed_by_second_worker (outbox.rs, all 3
    backends): worker A claims a short lease and "crashes"; after expiry worker B
    reclaims and completes; A's late complete/release are fenced (InvalidState).
    Exercises the SQL claimed_until expiry predicate end-to-end.
  - publish_failure_after_commit_retains_outbox_row_until_delivered (outbox.rs,
    all 3 backends): dispatcher with a publisher that fails twice then succeeds;
    row goes Pending after each failure (attempts incrementing), then Published,
    with exactly one bus delivery.
  - read_model_failure_mid_plan_rolls_back_events_and_outbox
    (sqlite_repository + postgres_repository): a commit with events + outbox + a
    two-mutation read-model plan whose second mutation hits a real DB constraint
    (SQLite trigger / Postgres CHECK); asserts events, outbox row, and the first
    mutation are all absent (whole-tx rollback).
  - unknown_event_type_in_stream_fails_hydration_with_clear_error +
    repo-path variant (upcasting): a stream with an unregistered event_name fails
    hydration/get with a RepositoryError::Replay naming the event.
  - queued_repo_n_writers_commit_in_fifo_order (queued_repo): 10 writers contend
    for one aggregate; asserts final version == N+1 and event order equals the
    lock-grant order.
  - replay determinism proptest (new tests/replay_property/): proptest over
    arbitrary command sequences — reload reproduces in-memory state; snapshot
    hydration at any frequency equals full replay.

  Adds proptest as a dev-dependency. Postgres-backed variants env-gate on
  DATABASE_URL and skip locally; SQLite/HashMap variants run without docker.

  Bugs discovered: none. Every test passes against current code.

  Implements [[tasks/red-team-conformance-tests]]

  Co-authored-by: Claude Fable 5 <noreply@anthropic.com>

* fix: route corrupt PostgresBus rows through failure policy (#80) (by @patrickleet)

  * fix: route corrupt PostgresBus rows through failure policy

  A `bus_queue`/`bus_log` row whose required columns (`name`, `kind`,
  `payload`) failed to decode was silently lost. `message_from_row` used
  `try_get(...).unwrap_or_default()`, so a corrupt row became a `Message`
  with an empty `name`. The runner classifies an empty/unrouted name as
  "no handler", which takes the ack-and-ignore path: `QueueReceived::ack`
  deletes the row and `LogReceived::ack` advances the consumer offset past
  it. The corrupt row vanished with no trace, bypassing the otherwise
  careful never-silently-drop failure policy.

  Silent-data-loss path:
    recv -> message_from_row (unwrap_or_default -> name = "")
         -> runner: router.handles(kind, "") == false
         -> received.ack()  (queue: DELETE row; log: advance offset)
         -> message gone, no dead-letter, no log

  Fix:
  - `message_from_row` returns `Result<Message, TransportError>`, raising a
    PERMANENT error when a required column is missing or fails to decode.
    `message_id`/`content_type`/`metadata` keep tolerant defaults (optional
    or schema-defaulted; they don't make a message unhandleable).
  - The Postgres `recv` claims the row/offset before decoding, so the claim
    must still be settled when decoding fails. `QueueReceived`/`LogReceived`
    carry the decode error and surface it via a new
    `ReceivedMessage::decode_error()` (defaults to `None`, so the other 8
    adapters are unchanged).
  - The runner checks `decode_error()` first and routes the claimed row
    through the configured `FailurePolicy` — dead-letter by default — exactly
    like a permanent dispatch failure. Queue corrupt rows are dead-lettered
    (deleted, logged), log corrupt rows advance the offset past the poison
    entry (don't get stuck), and the failure is visible instead of silent.

  Tests:
  - runner unit tests (no DB): a decode-error delivery with an unhandled
    (empty) name is dead-lettered under the default policy (not ack-and-
    ignored), parked under Park, and stops with a permanent error under Stop.
  - postgres_transport integration tests (env-gated on DATABASE_URL): a
    corrupt `bus_queue` row leaves the queue while the valid row beside it is
    handled; a corrupt `bus_log` row advances the consumer offset past it
    while the valid following event is handled.

  Implements [[tasks/postgres-bus-corrupt-row-handling]]

  * fix: claim corrupt name-NULL postgres bus rows so they settle

  The prior commit routed undecodable rows through the failure policy, but
  that path was unreachable for the most common corruption: a NULL `name`.

  Both sources select work by name — the queue claim filters
  `WHERE name = ANY($2)` and the log read filters `WHERE name = ANY($1)`.
  A row whose `name` column is NULL matches neither, so it is never claimed
  (queue) or never read (log). The decode-error handling never fired because
  the corrupt row was never even fetched. The queue row therefore sat
  undelivered forever (the failing test: queue still had 1 row), and a
  corrupt log entry was silently skipped when the offset jumped to a later
  healthy entry — dropped with no trace, the exact ack-and-ignore behavior
  the hardening set out to prevent.

  The missing piece was not settlement identity (the claim already captures
  `seq` independently of payload decode); it was *visibility*: an un-routable
  row must still be selectable so the runner can settle it.

  Fix: both sources also select rows with a NULL `name` (un-routable
  poison). Such a row belongs to no consumer, so claiming/reading it to
  dead-letter it is correct — `FOR UPDATE SKIP LOCKED` keeps the queue claim
  safe under competing listeners, and each log group advances its own offset
  past it. The runner then surfaces the decode error and routes it through
  the failure policy (dead-letter by default: the queue row is deleted, the
  log offset advances past the entry).

  Also strengthen the log test: it now places a corrupt entry as the highest
  seq, so a consumer that silently skips by name would leave the offset short
  of max_seq. Reaching max_seq proves the corrupt entries were settled
  through the policy, not skipped. Verified against a real Postgres 18
  (docker compose): the improved test fails without the LogSource fix and
  passes with it; full postgres_transport (9) and lib (253) suites green.

  Refines [[tasks/postgres-bus-corrupt-row-handling]]


See full diff: [v1.7.4...v1.7.5](https://github.com/hops-ops/distributed/compare/v1.7.4...v1.7.5)
