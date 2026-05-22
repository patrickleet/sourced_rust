use std::sync::Arc;

use crate::entity::{Committable, Entity};
use crate::lock::{InMemoryLockManager, Lock, LockError, LockManager};
use crate::repository::{
    Commit, CommitBatch, Get, GetMany, GetOne, RepositoryError, TransactionalCommit,
};
use crate::snapshot::{SnapshotRecord, SnapshotStore};

/// Options for read operations.
#[derive(Debug, Clone, Copy)]
pub struct ReadOpts {
    /// Whether to acquire a lock on the entity/entities.
    pub lock: bool,
}

impl Default for ReadOpts {
    fn default() -> Self {
        Self { lock: true }
    }
}

impl ReadOpts {
    /// Create options that skip locking.
    pub fn no_lock() -> Self {
        Self { lock: false }
    }
}

/// Repository wrapper that serializes access with process-local per-stream locks.
///
/// Locking reads (`get` and `get_many`) intentionally keep matching locks held
/// after returning. Call `commit` to release those locks after a successful
/// write, or call `abort`/`unlock` when the loaded entity is no longer being
/// written. Dropping a loaded entity without `commit` or `abort` leaves its
/// in-memory lock held until an explicit unlock.
///
/// Commit releases held locks only after the inner repository succeeds. On
/// commit errors, locks remain held so callers can inspect state, retry, or
/// explicitly abort.
pub struct QueuedRepository<R, L: LockManager = InMemoryLockManager> {
    inner: R,
    lock_manager: Arc<L>,
}

impl<R: Clone, L: LockManager> Clone for QueuedRepository<R, L> {
    fn clone(&self) -> Self {
        QueuedRepository {
            inner: self.inner.clone(),
            lock_manager: Arc::clone(&self.lock_manager),
        }
    }
}

impl<R> QueuedRepository<R> {
    pub fn new(inner: R) -> Self {
        QueuedRepository {
            inner,
            lock_manager: Arc::new(InMemoryLockManager::new()),
        }
    }
}

impl<R, L: LockManager> QueuedRepository<R, L> {
    /// Create a `QueuedRepository` with a custom lock manager.
    pub fn with_lock_manager(inner: R, lock_manager: L) -> Self {
        QueuedRepository {
            inner,
            lock_manager: Arc::new(lock_manager),
        }
    }

    /// Access the inner repository.
    pub fn inner(&self) -> &R {
        &self.inner
    }

    /// Access the lock manager.
    pub fn lock_manager(&self) -> &L {
        &self.lock_manager
    }

    pub fn lock(&self, id: impl AsRef<str>) -> Result<(), RepositoryError> {
        let id = id.as_ref();
        let lock = self.ensure_lock(id)?;
        if !lock.try_lock()? {
            return Err(LockError::AcquireFailed(format!("lock for {id} is already held")).into());
        }
        Ok(())
    }

    pub fn unlock(&self, id: impl AsRef<str>) -> Result<(), RepositoryError> {
        let lock = self.ensure_lock(id.as_ref())?;
        lock.unlock()?;
        Ok(())
    }

    pub fn abort(&self, id: impl AsRef<str>) -> Result<(), RepositoryError> {
        self.unlock(id)
    }

    fn ensure_lock(&self, id: &str) -> Result<Arc<L::Lock>, RepositoryError> {
        Ok(self.lock_manager.get_lock(id)?)
    }

    fn lock_ids_in_order(&self, ids: &[&str]) -> Result<Vec<Arc<L::Lock>>, RepositoryError> {
        let mut unique: Vec<&str> = ids.to_vec();
        unique.sort_unstable();
        unique.dedup();

        let mut locks = Vec::with_capacity(unique.len());
        for id in unique {
            let lock = self.ensure_lock(id)?;
            lock.lock()?;
            locks.push(lock);
        }

        Ok(locks)
    }
}

// ============================================================================
// Core trait implementations (with locking by default)
// ============================================================================

impl<R: GetOne, L: LockManager> GetOne for QueuedRepository<R, L> {
    fn get_one(&self, id: &str) -> Result<Option<Entity>, RepositoryError> {
        let lock = self.ensure_lock(id)?;
        lock.lock()?;
        self.inner.get_one(id)
    }
}

impl<R: GetMany + GetOne, L: LockManager> GetMany for QueuedRepository<R, L> {
    fn get_many(&self, ids: &[&str]) -> Result<Vec<Entity>, RepositoryError> {
        let _locks = self.lock_ids_in_order(ids)?;
        self.inner.get_many(ids)
    }
}

impl<R: Commit, L: LockManager> Commit for QueuedRepository<R, L> {
    fn commit<C: Committable + ?Sized>(&self, committable: &mut C) -> Result<(), RepositoryError> {
        let entities = committable.entities_mut();

        // Commit releases locks that were acquired by a prior locking read or
        // manual lock call. It does not acquire ownership itself because this
        // lock implementation has no guard token or owner tracking.
        let mut locks = Vec::with_capacity(entities.len());
        for entity in &entities {
            locks.push(self.ensure_lock(entity.id())?);
        }

        // Delegate to inner repository
        let result = self.inner.commit(committable);

        // Keep locks held on errors so callers can retry or explicitly abort.
        if result.is_ok() {
            for lock in locks {
                lock.unlock()?;
            }
        }

        result
    }
}

impl<R: TransactionalCommit, L: LockManager> TransactionalCommit for QueuedRepository<R, L> {
    fn commit_batch(&self, batch: CommitBatch<'_>) -> Result<(), RepositoryError> {
        let ids: Vec<&str> = batch.entities.iter().map(|entity| entity.id()).collect();
        // See `Commit::commit`: these handles are released after successful
        // inner commit and intentionally kept held on errors.
        let mut locks = Vec::with_capacity(ids.len());
        for id in ids {
            locks.push(self.ensure_lock(id)?);
        }

        let result = self.inner.commit_batch(batch);

        if result.is_ok() {
            for lock in locks {
                lock.unlock()?;
            }
        }

        result
    }
}

// ============================================================================
// WithOpts traits for opting out of locking
// ============================================================================

/// Get a single entity with options.
pub trait GetWithOpts: Get {
    fn get_with(&self, id: &str, opts: ReadOpts) -> Result<Option<Entity>, RepositoryError>;
}

/// Get multiple entities with options.
pub trait GetAllWithOpts: Get {
    fn get_all_with(&self, ids: &[&str], opts: ReadOpts) -> Result<Vec<Entity>, RepositoryError>;
}

impl<R: GetOne + GetMany, L: LockManager> GetWithOpts for QueuedRepository<R, L> {
    fn get_with(&self, id: &str, opts: ReadOpts) -> Result<Option<Entity>, RepositoryError> {
        if opts.lock {
            self.get_one(id)
        } else {
            self.inner.get_one(id)
        }
    }
}

impl<R: GetMany + GetOne, L: LockManager> GetAllWithOpts for QueuedRepository<R, L> {
    fn get_all_with(&self, ids: &[&str], opts: ReadOpts) -> Result<Vec<Entity>, RepositoryError> {
        if opts.lock {
            self.get_many(ids)
        } else {
            self.inner.get_many(ids)
        }
    }
}

// ============================================================================
// Unlock capability
// ============================================================================

/// Trait for repositories that support unlocking entities.
pub trait UnlockableRepository {
    fn unlock(&self, id: &str) -> Result<(), RepositoryError>;
}

impl<R, L: LockManager> UnlockableRepository for QueuedRepository<R, L> {
    fn unlock(&self, id: &str) -> Result<(), RepositoryError> {
        QueuedRepository::unlock(self, id)
    }
}

// ============================================================================
// SnapshotStore delegation
// ============================================================================

impl<R: SnapshotStore, L: LockManager> SnapshotStore for QueuedRepository<R, L> {
    fn get_snapshot(&self, id: &str) -> Result<Option<SnapshotRecord>, RepositoryError> {
        self.inner.get_snapshot(id)
    }

    fn save_snapshot(&self, record: SnapshotRecord) -> Result<(), RepositoryError> {
        self.inner.save_snapshot(record)
    }

    fn delete_snapshot(&self, id: &str) -> Result<bool, RepositoryError> {
        self.inner.delete_snapshot(id)
    }
}

/// Builder trait for wrapping a repository with queue locking.
pub trait Queueable: Sized {
    fn queued(self) -> QueuedRepository<Self> {
        QueuedRepository::new(self)
    }

    fn queued_with<L: LockManager>(self, lock_manager: L) -> QueuedRepository<Self, L> {
        QueuedRepository::with_lock_manager(self, lock_manager)
    }
}

impl<T> Queueable for T {}
