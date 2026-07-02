### What's changed in v2.2.4

* refactor: dedupe SQL lock managers into generic SqlxLockManager (#104) (by @patrickleet)

  The Postgres and SQLite lock managers were 75% line-identical clones:
  struct, new, the four with_* builders, migrate, sweep_expired, the whole
  cached-Arc get_lock body, and the 3-method Lock delegation differed only
  in pool type, owner prefix, and dialect SQL.

  Extract a generic SqlxLockManager<D>/SqlxLock<D> into sqlx_common,
  driven by a LockDialect trait carrying OWNER_PREFIX, DDL, ACQUIRE_SQL,
  RELEASE_SQL, SWEEP_SQL, a busy_is_contention hook (SQLite maps
  SQLITE_BUSY to contention; Postgres keeps the default), and a
  rows_affected accessor (sqlx has no dialect-neutral one). The sqlx
  bind/decode bounds live exactly once, on the blanket LeaseQueries impl,
  which also owns the single shared split-on-';' migrate loop.

  PostgresLockManager/SqliteLockManager and PostgresLock/SqliteLock are
  now type aliases over the marker dialects, so the public API is
  unchanged; the backend files shrink to dialect SQL consts plus the busy
  hook. This pilots the dialect-trait pattern for the other SQL modules.


  Claude-Session: https://claude.ai/code/session_01DzYSVLas93c7LbgHJWsW7n

  Co-authored-by: Claude Fable 5 <noreply@anthropic.com>


See full diff: [v2.2.3...v2.2.4](https://github.com/hops-ops/distributed/compare/v2.2.3...v2.2.4)
