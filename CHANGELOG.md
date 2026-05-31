### What's changed in v1.1.0

* chore: update readme (by @patrickleet)

* feat: durable SQLx lease lock for QueuedRepository (postgres + sqlite) (#50) (by @patrickleet)

  * feat!: durable SQLx lease lock for QueuedRepository (postgres + sqlite)

  Add PostgresLockManager / SqliteLockManager — durable, cross-process
  AsyncLockManager implementations backed by an `aggregate_locks` lease
  table — so a QueuedRepository can serialize per-aggregate access across
  processes, not just within one. Drop-in via `.queued_async_with(...)`.

  Each per-key lock layers an in-process gate (InMemoryAsyncLock) over the
  DB lease: same-process tasks serialize with true wakeups (no DB polling),
  and only the local winner contends cross-process. Acquire is a single
  atomic conditional upsert using the database clock (no cross-process
  skew); release is owner-token scoped so it never frees a holder that
  reclaimed an expired lease. It is a mutual-exclusion optimization, not a
  fence — the event-store sequence PK remains the authoritative boundary.
  v1 has no lease renewal; `sweep_expired` reclaims cold rows.

  BREAKING CHANGE: `AsyncLock::{try_lock,unlock}` are now async (a durable
  lock releases/acquires via I/O), which makes `AsyncUnlockableRepository`
  and `AsyncAggregateRepository::{abort,unlock}` async as well. Callers must
  `.await` these. The lock surface is now async-only.

  Implements [[tasks/persistent-lock-sqlx]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

  * fix(lock): address PR #50 review — cancellation safety, lazy ops, max_wait

  - Cancellation-safe in-process gate: a `GateGuard` releases the gate from
    `Drop`, so a `lease_lock`/`lease_try_lock`/`lease_unlock` future dropped
    mid-`await` (cancellation/timeout) no longer wedges the key. Replaces the
    explicit-error-path-only gate release.
  - `max_wait` now measured from entry and bounds the in-process gate wait too
    (was only applied to DB polling, after the gate was acquired).
  - In-memory `try_lock`/`unlock` are now lazy `async fn` (side effect runs on
    poll, not at call time) — consistent with the I/O-backed locks; a dropped,
    never-awaited future is a no-op. `unlock_core` is `pub(crate)` for the guard.
  - `AsyncAggregateRepository::abort` forwards to the repo's `abort` hook instead
    of `unlock`, so an `AsyncUnlockableRepository` overriding `abort` is honored.
  - Tests: add cancellation-safety regression (cancelled acquire releases the
    gate) on both backends.

  Migration-split suggestion intentionally not taken (owner decision): this crate
  re-runs the full idempotent `0001_initial.sql` on every `migrate()` — there is
  no applied-migration tracking — so adding `CREATE TABLE IF NOT EXISTS` to the
  baseline is the correct pattern here.

  Refs [[tasks/persistent-lock-sqlx]]

  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>

  ---------

  Co-authored-by: Claude Opus 4.8 (1M context) <noreply@anthropic.com>


See full diff: [v1.0.0...v1.1.0](https://github.com/patrickleet/sourced_rust/compare/v1.0.0...v1.1.0)
