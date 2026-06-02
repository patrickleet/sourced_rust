//! SQLite-backed durable [`AsyncLockManager`] — a per-stream lease in the
//! `aggregate_locks` table. Drop it into a `QueuedRepository` with
//! `.queued_with(SqliteLockManager::new(pool))`. See [`super::sqlx_common`]
//! for the lease model.
//!
//! Cross-process serialization requires the managers to share one database —
//! use a file or `cache=shared` URL, not a per-connection `:memory:` pool.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sqlx::SqlitePool;

use crate::sqlx_repo::is_sqlite_busy;

use super::sqlx_common::{
    default_owner_id, lease_acquire_error, lease_lock, lease_release_error, lease_try_lock,
    lease_unlock, mint_token, LeaseBackend, LeaseConfig, LockShared,
};
use super::{AsyncLock, AsyncLockManager, LockError};

/// Inline lease-table DDL for standalone [`SqliteLockManager::migrate`]. The same
/// table is created by `SqliteRepository`'s migrations; both are idempotent.
const SQLITE_LOCK_DDL: &str = "\
CREATE TABLE IF NOT EXISTS aggregate_locks (\
  lock_key TEXT NOT NULL PRIMARY KEY,\
  owner_token TEXT NOT NULL,\
  acquired_at REAL NOT NULL,\
  expires_at REAL NOT NULL,\
  CHECK (lock_key <> ''),\
  CHECK (owner_token <> '')\
);\
CREATE INDEX IF NOT EXISTS aggregate_locks_expires_at_idx ON aggregate_locks (expires_at);";

/// SQLite-backed [`AsyncLockManager`]. Hands out one cached [`SqliteLock`] per
/// key (like `InMemoryAsyncLockManager`).
///
/// Apply the `with_*` tunables **before the first [`get_lock`](AsyncLockManager::get_lock)**:
/// each per-key lock captures the configuration at creation time, so reconfiguring
/// after locks have been handed out would not affect the already-cached ones.
#[derive(Clone)]
pub struct SqliteLockManager {
    pool: SqlitePool,
    owner_id: String,
    config: LeaseConfig,
    token_seq: Arc<AtomicU64>,
    locks: Arc<Mutex<HashMap<String, Arc<SqliteLock>>>>,
}

impl SqliteLockManager {
    /// Create a manager over an existing (migrated) pool, with default tunables
    /// (30s lease TTL, 50ms retry interval, wait indefinitely).
    pub fn new(pool: SqlitePool) -> Self {
        Self {
            pool,
            owner_id: default_owner_id("sqlite"),
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
    pub fn with_owner_id(mut self, owner_id: impl Into<String>) -> Self {
        self.owner_id = owner_id.into();
        self
    }

    /// Create the `aggregate_locks` table for standalone use (no repository).
    pub async fn migrate(pool: &SqlitePool) -> Result<(), LockError> {
        for statement in SQLITE_LOCK_DDL.split(';') {
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

    /// Delete all expired lease rows; returns how many were reclaimed.
    pub async fn sweep_expired(&self) -> Result<u64, LockError> {
        let result = sqlx::query(
            "DELETE FROM aggregate_locks WHERE expires_at <= unixepoch('now','subsec')",
        )
        .execute(&self.pool)
        .await
        .map_err(lease_release_error)?;
        Ok(result.rows_affected())
    }
}

impl AsyncLockManager for SqliteLockManager {
    type Lock = SqliteLock;

    fn get_lock(&self, id: &str) -> Result<Arc<SqliteLock>, LockError> {
        let mut locks = self
            .locks
            .lock()
            .map_err(|_| LockError::Poisoned("sqlite lock manager map poisoned".into()))?;
        Ok(locks
            .entry(id.to_string())
            .or_insert_with(|| {
                Arc::new(SqliteLock {
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

/// A single SQLite lease lock for one stream key.
pub struct SqliteLock {
    pool: SqlitePool,
    owner_id: String,
    config: LeaseConfig,
    token_seq: Arc<AtomicU64>,
    key: String,
    shared: LockShared,
}

impl LeaseBackend for SqliteLock {
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
        // Same conditional upsert as Postgres (insert / steal-expired /
        // re-acquire-own-token), one atomic statement. SQLite serializes writers,
        // so a colliding writer may get SQLITE_BUSY — treat that as contention
        // (retry), not failure, since the pool sets no busy_timeout.
        let ttl = self.config.lease_ttl.as_secs_f64();
        let outcome = sqlx::query_as::<_, (String,)>(
            r#"
            INSERT INTO aggregate_locks (lock_key, owner_token, acquired_at, expires_at)
            VALUES (?1, ?2, unixepoch('now','subsec'), unixepoch('now','subsec') + ?3)
            ON CONFLICT (lock_key) DO UPDATE
              SET owner_token = excluded.owner_token,
                  acquired_at = excluded.acquired_at,
                  expires_at = excluded.expires_at
              WHERE aggregate_locks.expires_at <= unixepoch('now','subsec')
                 OR aggregate_locks.owner_token = excluded.owner_token
            RETURNING owner_token
            "#,
        )
        .bind(&self.key)
        .bind(token)
        .bind(ttl)
        .fetch_optional(&self.pool)
        .await;
        match outcome {
            Ok(row) => Ok(matches!(row, Some((owner,)) if owner == token)),
            Err(err) if is_sqlite_busy(&err) => Ok(false),
            Err(err) => Err(lease_acquire_error(err)),
        }
    }

    async fn db_release(&self, token: &str) -> Result<(), LockError> {
        sqlx::query("DELETE FROM aggregate_locks WHERE lock_key = ?1 AND owner_token = ?2")
            .bind(&self.key)
            .bind(token)
            .execute(&self.pool)
            .await
            .map_err(lease_release_error)?;
        Ok(())
    }
}

impl AsyncLock for SqliteLock {
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
