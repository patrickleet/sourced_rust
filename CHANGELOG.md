### What's changed in v2.3.5

* refactor: collapse postgres/sqlite repositories into shared generic layer (#111) (by @patrickleet)

  Extends the existing SqlxReadModelBackend / lock/sqlx_common dialect-trait
  pattern to the event-store/snapshot/outbox/inbox layers. postgres_repo and
  sqlite_repo shrink from ~1700 lines each to ~460-line dialect shims over a
  shared SqlxRepository/SqlxOutboxStore in src/sqlx_repo/repo.rs. Outbox `claim`
  stays per-backend (postgres SKIP LOCKED CTE vs sqlite scan-loop).

  Squashed from 14 commits for a single rebase reconciliation onto main (after

  - fix: surface malformed sqlite timestamps as errors (was silent UNIX_EPOCH);
    align null-bind error message with postgres (includes field_name).
  - refactor: share commit-batch validation across all backends (fixes hashmap
    vs SQL snapshot-identity drift).
  - feat!: widen postgres integer columns to BIGINT so both backends decode i64;
    deletes the width-conversion helpers.
  - perf: batch snapshot loads for get_all hydration (fixes N+1); get_streams is
    a GetStream default method.
  - perf: batch the commit_batch concurrency pre-check into one query.
  - perf!: borrow events in PreparedEventAppend instead of cloning.
  - feat!: bound outbox status listings (messages_by_status/pending) with a limit.
  - perf!: store EventRecord.payload_codec as Cow<'static, str>.
  - feat: run migrations through sqlx's Migrator with a _sqlx_migrations ledger.
  - fix: chunk postgres event/outbox inserts under the 65535 bind-param cap.
  - fix: classify postgres 57P01/57P02/57P03 and class-08 SQLSTATEs as transient.

  Merge-reconciliation notes:
  - table/ is now the canonical vocabulary (#109); backend shims import the
    renamed types via local aliases (TableColumn as ColumnDef, TableStoreError
    as ReadModelError) to keep the collapsed bodies unchanged.
  - #106's batched OutboxStore::complete_many overrides lived in the old backend
    files and were collapsed away; SqlxOutboxStore inherits the serial default
    (correct, conformance-tested). Re-adding a batched override to the shared
    layer is a follow-up (see tasks/outbox-sqlx-batched-complete-many).

  REBASED onto main+#110 (9f34f0a): #110 already merged, so its fault-injection
  test that pinned the OLD 57P01 (mis)classification is flipped here to
  `assert!(err.is_retryable())` to match this PR's classification fix; #110's
  shared outbox test helpers are updated for the bounded messages_by_status(limit)
  API. Main stays green on merge.


  Claude-Session: https://claude.ai/code/session_01DzYSVLas93c7LbgHJWsW7n

  Co-authored-by: Claude Fable 5 <noreply@anthropic.com>


See full diff: [v2.3.4...v2.3.5](https://github.com/hops-ops/distributed/compare/v2.3.4...v2.3.5)
