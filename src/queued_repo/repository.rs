use std::sync::Arc;

use crate::lock::{InMemoryLockManager, Lock, LockManager};
use crate::entity::{Committable, Entity};
use crate::repository::{
    Commit, Count, Exists, Find, FindOne, Get, GetMany, GetOne, RepositoryError,
};

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

pub struct QueuedRepository<R, L: LockManager = InMemoryLockManager> {
    inner: R,
    lock_manager: L,
}

impl<R> QueuedRepository<R> {
    pub fn new(inner: R) -> Self {
        QueuedRepository {
            inner,
            lock_manager: InMemoryLockManager::new(),
        }
    }
}

impl<R, L: LockManager> QueuedRepository<R, L> {
    /// Create a `QueuedRepository` with a custom lock manager.
    pub fn with_lock_manager(inner: R, lock_manager: L) -> Self {
        QueuedRepository {
            inner,
            lock_manager,
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
        let lock = self.ensure_lock(id.as_ref())?;
        let _ = lock.try_lock()?;
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
        let mut unique: Vec<&str> = ids.iter().copied().collect();
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

impl<R: Find + GetOne, L: LockManager> Find for QueuedRepository<R, L> {
    fn find<F>(&self, predicate: F) -> Result<Vec<Entity>, RepositoryError>
    where
        F: Fn(&Entity) -> bool,
    {
        // First, find matching entities without locks
        let entities = self.inner.find(&predicate)?;

        // Lock all matching entity IDs
        let ids: Vec<&str> = entities.iter().map(|e| e.id()).collect();
        let _locks = self.lock_ids_in_order(&ids)?;

        // Re-fetch with locks held to ensure consistency
        let mut results = Vec::with_capacity(entities.len());
        for id in ids {
            if let Some(entity) = self.inner.get_one(id)? {
                if predicate(&entity) {
                    results.push(entity);
                }
            }
        }
        Ok(results)
    }
}

impl<R: FindOne + GetOne, L: LockManager> FindOne for QueuedRepository<R, L> {
    fn find_one<F>(&self, predicate: F) -> Result<Option<Entity>, RepositoryError>
    where
        F: Fn(&Entity) -> bool,
    {
        // First, find a matching entity without lock
        let entity = self.inner.find_one(&predicate)?;

        if let Some(entity) = entity {
            // Lock the entity
            let lock = self.ensure_lock(entity.id())?;
            lock.lock()?;

            // Re-fetch with lock held to ensure consistency
            if let Some(entity) = self.inner.get_one(entity.id())? {
                if predicate(&entity) {
                    return Ok(Some(entity));
                }
            }
            // Entity no longer matches, unlock
            lock.unlock()?;
        }
        Ok(None)
    }
}

impl<R: Exists, L: LockManager> Exists for QueuedRepository<R, L> {
    /// Check if any entity matches (non-locking - just a read check).
    fn exists<F>(&self, predicate: F) -> Result<bool, RepositoryError>
    where
        F: Fn(&Entity) -> bool,
    {
        self.inner.exists(predicate)
    }
}

impl<R: Count, L: LockManager> Count for QueuedRepository<R, L> {
    /// Count matching entities (non-locking - just a read check).
    fn count<F>(&self, predicate: F) -> Result<usize, RepositoryError>
    where
        F: Fn(&Entity) -> bool,
    {
        self.inner.count(predicate)
    }
}

impl<R: Commit, L: LockManager> Commit for QueuedRepository<R, L> {
    fn commit<C: Committable + ?Sized>(&self, committable: &mut C) -> Result<(), RepositoryError> {
        let entities = committable.entities_mut();

        // Acquire locks for all entities
        let mut locks = Vec::with_capacity(entities.len());
        for entity in &entities {
            locks.push(self.ensure_lock(entity.id())?);
        }

        // Delegate to inner repository
        let result = self.inner.commit(committable);

        // Unlock on success
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

/// Find entities with options.
pub trait FindWithOpts: Find {
    fn find_with<F>(&self, predicate: F, opts: ReadOpts) -> Result<Vec<Entity>, RepositoryError>
    where
        F: Fn(&Entity) -> bool;
}

/// Find one entity with options.
pub trait FindOneWithOpts: FindOne {
    fn find_one_with<F>(
        &self,
        predicate: F,
        opts: ReadOpts,
    ) -> Result<Option<Entity>, RepositoryError>
    where
        F: Fn(&Entity) -> bool;
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

impl<R: Find + GetOne, L: LockManager> FindWithOpts for QueuedRepository<R, L> {
    fn find_with<F>(&self, predicate: F, opts: ReadOpts) -> Result<Vec<Entity>, RepositoryError>
    where
        F: Fn(&Entity) -> bool,
    {
        if opts.lock {
            self.find(predicate)
        } else {
            self.inner.find(predicate)
        }
    }
}

impl<R: FindOne + GetOne, L: LockManager> FindOneWithOpts for QueuedRepository<R, L> {
    fn find_one_with<F>(
        &self,
        predicate: F,
        opts: ReadOpts,
    ) -> Result<Option<Entity>, RepositoryError>
    where
        F: Fn(&Entity) -> bool,
    {
        if opts.lock {
            self.find_one(predicate)
        } else {
            self.inner.find_one(predicate)
        }
    }
}

// ============================================================================
// Legacy traits (kept for compatibility)
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
