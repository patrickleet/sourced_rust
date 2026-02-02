use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::lock::Lock;
use crate::{Entity, Repository, RepositoryError};

pub struct QueuedRepository<R> {
    inner: R,
    locks: Mutex<HashMap<String, Arc<Lock>>>,
}

impl<R> QueuedRepository<R> {
    pub fn new(inner: R) -> Self {
        QueuedRepository {
            inner,
            locks: Mutex::new(HashMap::new()),
        }
    }

    pub fn lock(&self, id: impl AsRef<str>) -> Result<(), RepositoryError> {
        let lock = self.ensure_lock(id.as_ref())?;
        let _ = lock.try_lock();
        Ok(())
    }

    pub fn unlock(&self, id: impl AsRef<str>) -> Result<(), RepositoryError> {
        let lock = self.ensure_lock(id.as_ref())?;
        lock.unlock();
        Ok(())
    }

    pub fn abort(&self, id: impl AsRef<str>) -> Result<(), RepositoryError> {
        self.unlock(id)
    }

    fn ensure_lock(&self, id: &str) -> Result<Arc<Lock>, RepositoryError> {
        let mut locks = self
            .locks
            .lock()
            .map_err(|_| RepositoryError::LockPoisoned("queue map"))?;
        Ok(locks
            .entry(id.to_string())
            .or_insert_with(|| Arc::new(Lock::new()))
            .clone())
    }

    fn lock_ids_in_order(&self, ids: &[&str]) -> Result<Vec<Arc<Lock>>, RepositoryError> {
        let mut unique: Vec<&str> = ids.iter().copied().collect();
        unique.sort_unstable();
        unique.dedup();

        let mut locks = Vec::with_capacity(unique.len());
        for id in unique {
            let lock = self.ensure_lock(id)?;
            lock.lock();
            locks.push(lock);
        }

        Ok(locks)
    }
}

impl<R: Repository> QueuedRepository<R> {
    // Read without taking the queue lock; may return stale data during in-flight writes.
    pub fn peek(&self, id: &str) -> Result<Option<Entity>, RepositoryError> {
        self.inner.get(id)
    }

    // Read without taking the queue lock; may return stale data during in-flight writes.
    pub fn peek_all(&self, ids: &[&str]) -> Result<Vec<Entity>, RepositoryError> {
        self.inner.get_all(ids)
    }
}

impl<R: Repository> Repository for QueuedRepository<R> {
    fn get(&self, id: &str) -> Result<Option<Entity>, RepositoryError> {
        let lock = self.ensure_lock(id)?;
        lock.lock();
        self.inner.get(id)
    }

    fn commit(&self, entity: &mut Entity) -> Result<(), RepositoryError> {
        let lock = self.ensure_lock(entity.id())?;
        let result = self.inner.commit(entity);
        if result.is_ok() {
            lock.unlock();
        }
        result
    }

    fn get_all(&self, ids: &[&str]) -> Result<Vec<Entity>, RepositoryError> {
        let _locks = self.lock_ids_in_order(ids)?;
        self.inner.get_all(ids)
    }

    fn commit_all(&self, entities: &mut [&mut Entity]) -> Result<(), RepositoryError> {
        let ids: Vec<&str> = entities.iter().map(|entity| entity.id()).collect();
        let mut locks = Vec::with_capacity(ids.len());
        for id in ids {
            locks.push(self.ensure_lock(id)?);
        }

        let result = self.inner.commit_all(entities);
        if result.is_ok() {
            for lock in locks {
                lock.unlock();
            }
        }

        result
    }
}

pub trait Queueable: Repository + Sized {
    fn queued(self) -> QueuedRepository<Self> {
        QueuedRepository::new(self)
    }
}

impl<T: Repository> Queueable for T {}
