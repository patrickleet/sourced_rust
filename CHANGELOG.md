### What's changed in v1.8.0

* feat: give RepositoryError and LockError a retryability signal (#83) (by @patrickleet)

  * refactor!: give RepositoryError and LockError a retryability signal

  repository_storage_error mapped every sqlx failure — connection refused,
  pool timeout, SQLITE_BUSY, and constraint violations alike — to
  RepositoryError::Model(String). RepositoryError had no storage shape and no
  source(), and From<ReadModelError>/From<EventRecordError> collapsed to
  strings. Downstream, HandlerError::transport_error_kind classified every
  Repository(_) as Retryable, so a deterministic model error redelivered
  forever while a transient DB outage was indistinguishable from it.

  Add a Storage { operation, retryable, source } variant to RepositoryError
  plus a kind() -> RetryClass method, mirroring TransportError. RetryClass
  lives in the lock module (the lowest layer both errors reach) so repository
  does not depend on the bus. repository_storage_error now classifies sqlx
  errors via is_sqlx_transient (connection/pool/timeout and SQLITE_BUSY ->
  retryable; constraint/decode -> permanent) and preserves the source. The
  From impls map to Storage, keeping the source and the transient/permanent
  distinction. ConcurrentWrite and NotFound stay retryable (race resolved by
  a later attempt); Model/Replay/invalid-identity/InvalidState are permanent.

  HandlerError::transport_error_kind now defers to RepositoryError::kind()
  instead of bucketing all Repository(_) as retryable.

  LockError gains a Busy variant and the same kind()/is_retryable() signal:
  Poisoned is permanent (broken invariant); contention, lease loss, and busy
  are retryable. The existing AcquireFailed/ReleaseFailed lease mappings were
  already retryable under this classification.

  Storage carries a boxed source, so RepositoryError drops Clone/PartialEq/Eq
  (they were test-only). The handful of assert_eq! comparisons became
  matches! on the variant. Both enums are now #[non_exhaustive] so future
  variants are non-breaking.

  BREAKING CHANGE: RepositoryError no longer derives Clone/PartialEq/Eq, both
  RepositoryError and LockError are #[non_exhaustive], and From<ReadModelError>
  /From<EventRecordError> now produce RepositoryError::Storage instead of
  ::Model.

  Implements [[tasks/repository-error-taxonomy]]

  * test(knative): use a genuinely retryable error for the 503 redeliver case

  The `retryable_failure_returns_503` ingress test handler returned
  `RepositoryError::Model("transient")`. After the error-taxonomy rework,
  `Model` is — correctly — a deterministic fault classified Permanent, so
  the ingress mapped it to 422 (do-not-retry), failing the assertion.

  This was a stale expectation asserting the old redeliver-forever bug, not
  a misclassification: a `Model` error is deterministic and re-running the
  identical message cannot change its outcome, so it MUST be permanent (the
  `deterministic_repository_errors_are_permanent` unit test pins this).

  The fix is in the test, not the classification logic. The handler now
  returns `RepositoryError::retryable_storage("load stream", ...)` — a
  transient storage outage (connection refused) that genuinely may succeed
  on redelivery — which is what a "temporarily failed" handler should
  signal. The 503 redeliver semantics the test names are now honestly
  exercised, and the deterministic-permanent path is covered by
  `order.rejected` -> 422.

  Refines [[tasks/repository-error-taxonomy]]

  * test: expect permanent Storage error for read-model constraint violation

  A read-model CHECK-constraint violation is a deterministic, non-retryable
  fault. With the reworked error taxonomy it now surfaces as the new
  `RepositoryError::Storage { retryable: false, .. }` variant rather than the
  old `RepositoryError::Model(String)`, so re-running the identical commit is
  never retried forever.

  PR #85 added `read_model_failure_mid_plan_rolls_back_events_and_outbox` to
  both the sqlite and postgres repository suites asserting the old `Model`
  error. Update both assertions to expect the permanent `Storage` variant
  (and `!err.is_retryable()`), matching the corrected classification and the
  idiom already used by this PR's own backend-storage tests. The rollback
  invariant the test actually guards — events, outbox row, and the first
  read-model mutation all absent after the failed commit — is unchanged.

  Refines [[tasks/repository-error-taxonomy]]

  * fix(repo): classify Postgres deadlock/serialization SQLSTATEs as retryable

  40001 (serialization_failure) and 40P01 (deadlock_detected) are transient
  write-race outcomes that should be retried, not handed to the failure policy.
  is_sqlx_transient previously only matched pool/timeout/IO and SQLite busy, so
  these fell through as permanent. Mirrors the existing 23505 unique-violation
  detection (no feature gate: SQLite never carries these SQLSTATEs).

  Addresses CodeRabbit review on PR #83.

  Refines [[tasks/repository-error-taxonomy]]


See full diff: [v1.7.6...v1.8.0](https://github.com/hops-ops/distributed/compare/v1.7.6...v1.8.0)
