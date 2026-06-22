//! Postgres-backed durable [`LockManager`] — a per-stream lease in the
//! `aggregate_locks` table. Drop it into a `QueuedRepository` with
//! `.queued_with(PostgresLockManager::new(pool))` to serialize aggregate
//! access across processes. See [`super::sqlx_common`] for the lease model.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sqlx::PgPool;

use super::sqlx_common::{
    default_owner_id, lease_acquire_error, lease_lock, lease_release_error, lease_try_lock,
    lease_unlock, mint_token, LeaseBackend, LeaseConfig, LockShared,
};
use super::{Lock, LockError, LockManager};

/// Inline lease-table DDL for standalone [`PostgresLockManager::migrate`]. The
/// same table is created by `PostgresRepository`'s migrations; both are
/// idempotent.
const PG_LOCK_DDL: &str = "\
CREATE TABLE IF NOT EXISTS aggregate_locks (\
  lock_key text NOT NULL PRIMARY KEY,\
  owner_token text NOT NULL,\
  acquired_at double precision NOT NULL,\
  expires_at double precision NOT NULL,\
  CHECK (lock_key <> ''),\
  CHECK (owner_token <> '')\
);\
CREATE INDEX IF NOT EXISTS aggregate_locks_expires_at_idx ON aggregate_locks (expires_at);";

/// Postgres-backed [`LockManager`]. Hands out one cached [`PostgresLock`]
/// per key (like `InMemoryLockManager`).
///
/// Apply the `with_*` tunables **before the first [`get_lock`](LockManager::get_lock)**:
/// each per-key lock captures the configuration at creation time, so reconfiguring
/// after locks have been handed out would not affect the already-cached ones.
#[derive(Clone)]
pub struct PostgresLockManager {
    pool: PgPool,
    owner_id: String,
    config: LeaseConfig,
    token_seq: Arc<AtomicU64>,
    locks: Arc<Mutex<HashMap<String, Arc<PostgresLock>>>>,
}

impl PostgresLockManager {
    /// Create a manager over an existing (migrated) pool, with default tunables
    /// (30s lease TTL, 50ms retry interval, wait indefinitely).
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            owner_id: default_owner_id("pg"),
            config: LeaseConfig::default(),
            token_seq: Arc::new(AtomicU64::new(0)),
            locks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Set how long an acquired lease stays valid before it becomes stealable.
    /// Must exceed the worst-case critical section (v1 has no renewal).
    pub fn with_lease_ttl(mut self, ttl: Duration) -> Self {
        self.config.lease_ttl = ttl;
        self
    }

    /// Set the wait between contended acquire attempts.
    pub fn with_retry_interval(mut self, interval: Duration) -> Self {
        self.config.retry_interval = interval;
        self
    }

    /// Cap how long `lock` waits before failing with `AcquireFailed`. `None`
    /// (the default) waits indefinitely.
    pub fn with_max_wait(mut self, max_wait: Option<Duration>) -> Self {
        self.config.max_wait = max_wait;
        self
    }

    /// Override the owner id (otherwise a process-unique id is generated).
    /// Useful for observability; must stay unique per process.
    pub fn with_owner_id(mut self, owner_id: impl Into<String>) -> Self {
        self.owner_id = owner_id.into();
        self
    }

    /// Create the `aggregate_locks` table for standalone use (no repository).
    pub async fn migrate(pool: &PgPool) -> Result<(), LockError> {
        for statement in PG_LOCK_DDL.split(';') {
            let statement = statement.trim();
            if statement.is_empty() {
                continue;
            }
            sqlx::query(statement).execute(pool).await.map_err(|err| {
                LockError::Other(format!("migrate aggregate_locks failed: {err}"))
            })?;
        }
        Ok(())
    }

    /// Delete all expired lease rows; returns how many were reclaimed. Optional
    /// GC for keys that were locked once and never again (acquire already reuses
    /// the row for live keys).
    pub async fn sweep_expired(&self) -> Result<u64, LockError> {
        let result = sqlx::query(
            "DELETE FROM aggregate_locks WHERE expires_at <= extract(epoch from now())",
        )
        .execute(&self.pool)
        .await
        .map_err(lease_release_error)?;
        Ok(result.rows_affected())
    }
}

impl LockManager for PostgresLockManager {
    type Lock = PostgresLock;

    fn get_lock(&self, id: &str) -> Result<Arc<PostgresLock>, LockError> {
        let mut locks = self
            .locks
            .lock()
            .map_err(|_| LockError::Poisoned("postgres lock manager map poisoned".into()))?;
        Ok(locks
            .entry(id.to_string())
            .or_insert_with(|| {
                Arc::new(PostgresLock {
                    pool: self.pool.clone(),
                    owner_id: self.owner_id.clone(),
                    config: self.config.clone(),
                    token_seq: Arc::clone(&self.token_seq),
                    key: id.to_string(),
                    shared: LockShared::new(),
                })
            })
            .clone())
    }
}

/// A single Postgres lease lock for one stream key.
pub struct PostgresLock {
    pool: PgPool,
    owner_id: String,
    config: LeaseConfig,
    token_seq: Arc<AtomicU64>,
    key: String,
    shared: LockShared,
}

impl LeaseBackend for PostgresLock {
    fn shared(&self) -> &LockShared {
        &self.shared
    }

    fn config(&self) -> &LeaseConfig {
        &self.config
    }

    fn mint_token(&self) -> String {
        mint_token(&self.owner_id, &self.token_seq)
    }

    async fn db_acquire(&self, token: &str) -> Result<bool, LockError> {
        // Atomic conditional upsert: insert when absent, or steal when the lease
        // expired, or re-acquire our own token (idempotent on a lost-response
        // retry). A holder that is present and unexpired makes the DO UPDATE a
        // no-op, so RETURNING yields no row — without raising a unique violation.
        let ttl = self.config.lease_ttl.as_secs_f64();
        let row: Option<(String,)> = sqlx::query_as(
            r#"
            INSERT INTO aggregate_locks (lock_key, owner_token, acquired_at, expires_at)
            VALUES ($1, $2, extract(epoch from now()), extract(epoch from now()) + $3)
            ON CONFLICT (lock_key) DO UPDATE
              SET owner_token = EXCLUDED.owner_token,
                  acquired_at = EXCLUDED.acquired_at,
                  expires_at = EXCLUDED.expires_at
              WHERE aggregate_locks.expires_at <= extract(epoch from now())
                 OR aggregate_locks.owner_token = EXCLUDED.owner_token
            RETURNING owner_token
            "#,
        )
        .bind(&self.key)
        .bind(token)
        .bind(ttl)
        .fetch_optional(&self.pool)
        .await
        .map_err(lease_acquire_error)?;
        Ok(matches!(row, Some((owner,)) if owner == token))
    }

    async fn db_release(&self, token: &str) -> Result<(), LockError> {
        sqlx::query("DELETE FROM aggregate_locks WHERE lock_key = $1 AND owner_token = $2")
            .bind(&self.key)
            .bind(token)
            .execute(&self.pool)
            .await
            .map_err(lease_release_error)?;
        Ok(())
    }
}

impl Lock for PostgresLock {
    fn lock(&self) -> impl Future<Output = Result<(), LockError>> + Send + '_ {
        lease_lock(self)
    }

    fn try_lock(&self) -> impl Future<Output = Result<bool, LockError>> + Send + '_ {
        lease_try_lock(self)
    }

    fn unlock(&self) -> impl Future<Output = Result<(), LockError>> + Send + '_ {
        lease_unlock(self)
    }
}
