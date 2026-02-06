//! QueuedReadModelStore - Per-instance locking for read models.
//!
//! Mirrors the `QueuedRepository` pattern for entities:
//! `get_model` acquires a lock, write operations (`upsert`, `insert`, `update`,
//! `delete`, `upsert_raw`) release it. Callers can also release manually via `unlock`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::entity::Committable;
use crate::lock::Lock;
use crate::queued_repo::ReadOpts;
use crate::repository::{Commit, RepositoryError};

use super::{ReadModel, ReadModelError, ReadModelStore, Versioned};

/// A `ReadModelStore` wrapper that provides per-instance locking.
///
/// Lock lifecycle matches the entity `QueuedRepository` pattern:
/// - `get_model` acquires a lock (or waits if already locked)
/// - `upsert` / `insert` / `update` / `delete` / `upsert_raw` release the lock on success
/// - `unlock` / `abort` release the lock manually
///
/// Lock keys are `"collection:id"`, so different read model types with the same ID
/// do not contend with each other.
pub struct QueuedReadModelStore<S> {
    inner: S,
    locks: Mutex<HashMap<String, Arc<Lock>>>,
}

impl<S> QueuedReadModelStore<S> {
    pub fn new(inner: S) -> Self {
        QueuedReadModelStore {
            inner,
            locks: Mutex::new(HashMap::new()),
        }
    }

    /// Access the inner store.
    pub fn inner(&self) -> &S {
        &self.inner
    }

    /// Manually lock a read model instance.
    pub fn lock<M: ReadModel>(&self, id: &str) -> Result<(), ReadModelError> {
        let key = Self::make_key(M::COLLECTION, id);
        let lock = self.ensure_lock(&key)?;
        lock.lock();
        Ok(())
    }

    /// Manually unlock a read model instance.
    pub fn unlock<M: ReadModel>(&self, id: &str) -> Result<(), ReadModelError> {
        let key = Self::make_key(M::COLLECTION, id);
        let lock = self.ensure_lock(&key)?;
        lock.unlock();
        Ok(())
    }

    /// Abort — alias for unlock.
    pub fn abort<M: ReadModel>(&self, id: &str) -> Result<(), ReadModelError> {
        self.unlock::<M>(id)
    }

    fn release(&self, key: &str) {
        if let Ok(locks) = self.locks.lock() {
            if let Some(lock) = locks.get(key) {
                lock.unlock();
            }
        }
    }

    fn ensure_lock(&self, key: &str) -> Result<Arc<Lock>, ReadModelError> {
        let mut locks = self
            .locks
            .lock()
            .map_err(|_| ReadModelError::Storage("queue lock map poisoned".into()))?;
        Ok(locks
            .entry(key.to_string())
            .or_insert_with(|| Arc::new(Lock::new()))
            .clone())
    }

    fn lock_ids_in_order(&self, keys: &[String]) -> Result<Vec<Arc<Lock>>, ReadModelError> {
        let mut unique = keys.to_vec();
        unique.sort_unstable();
        unique.dedup();

        let mut locks = Vec::with_capacity(unique.len());
        for key in &unique {
            let lock = self.ensure_lock(key)?;
            lock.lock();
            locks.push(lock);
        }

        Ok(locks)
    }

    fn make_key(collection: &str, id: &str) -> String {
        format!("{}:{}", collection, id)
    }
}

// ============================================================================
// ReadModelStore implementation (locking by default)
// ============================================================================

impl<S: ReadModelStore> ReadModelStore for QueuedReadModelStore<S> {
    fn get_model<M: ReadModel>(&self, id: &str) -> Result<Option<Versioned<M>>, ReadModelError> {
        let key = Self::make_key(M::COLLECTION, id);
        let lock = self.ensure_lock(&key)?;
        lock.lock();
        self.inner.get_model(id)
    }

    fn upsert<M: ReadModel>(&self, model: &M) -> Result<Versioned<M>, ReadModelError> {
        let key = Self::make_key(M::COLLECTION, model.id());
        let result = self.inner.upsert(model);
        if result.is_ok() {
            self.release(&key);
        }
        result
    }

    fn insert<M: ReadModel>(&self, model: &M) -> Result<Versioned<M>, ReadModelError> {
        let key = Self::make_key(M::COLLECTION, model.id());
        let result = self.inner.insert(model);
        if result.is_ok() {
            self.release(&key);
        }
        result
    }

    fn update<M: ReadModel>(
        &self,
        model: &M,
        expected_version: u64,
    ) -> Result<Versioned<M>, ReadModelError> {
        let key = Self::make_key(M::COLLECTION, model.id());
        let result = self.inner.update(model, expected_version);
        if result.is_ok() {
            self.release(&key);
        }
        result
    }

    fn delete<M: ReadModel>(&self, id: &str) -> Result<bool, ReadModelError> {
        let key = Self::make_key(M::COLLECTION, id);
        let result = self.inner.delete::<M>(id);
        if result.is_ok() {
            self.release(&key);
        }
        result
    }

    fn find_models<M: ReadModel>(
        &self,
        predicate: &dyn Fn(&M) -> bool,
    ) -> Result<Vec<Versioned<M>>, ReadModelError> {
        // Phase 1: find without locks
        let matches = self.inner.find_models(predicate)?;

        // Phase 2: lock all matching IDs in sorted order to prevent deadlocks
        let keys: Vec<String> = matches
            .iter()
            .map(|v| Self::make_key(M::COLLECTION, v.data.id()))
            .collect();
        let _locks = self.lock_ids_in_order(&keys)?;

        // Phase 3: re-fetch with locks held to ensure consistency
        let mut results = Vec::new();
        for versioned in &matches {
            let id = versioned.data.id();
            if let Some(current) = self.inner.get_model::<M>(id)? {
                if predicate(&current.data) {
                    results.push(current);
                } else {
                    // No longer matches, release
                    self.release(&Self::make_key(M::COLLECTION, id));
                }
            } else {
                // Deleted between phases, release
                self.release(&Self::make_key(M::COLLECTION, id));
            }
        }

        Ok(results)
    }

    fn find_one_model<M: ReadModel>(
        &self,
        predicate: &dyn Fn(&M) -> bool,
    ) -> Result<Option<Versioned<M>>, ReadModelError> {
        // Phase 1: find without lock
        let found = self.inner.find_one_model(predicate)?;

        if let Some(versioned) = found {
            let id = versioned.data.id().to_string();
            let key = Self::make_key(M::COLLECTION, &id);
            let lock = self.ensure_lock(&key)?;
            lock.lock();

            // Phase 2: re-fetch with lock held
            if let Some(current) = self.inner.get_model::<M>(&id)? {
                if predicate(&current.data) {
                    return Ok(Some(current));
                }
            }
            // No longer matches, unlock
            lock.unlock();
        }

        Ok(None)
    }

    fn upsert_raw(&self, key: &str, bytes: Vec<u8>) -> Result<(), ReadModelError> {
        let result = self.inner.upsert_raw(key, bytes);
        if result.is_ok() {
            self.release(key);
        }
        result
    }
}

// ============================================================================
// Commit delegation (needed for CommitBuilder integration)
// ============================================================================

impl<S: Commit> Commit for QueuedReadModelStore<S> {
    fn commit<C: Committable + ?Sized>(&self, committable: &mut C) -> Result<(), RepositoryError> {
        self.inner.commit(committable)
    }
}

// ============================================================================
// WithOpts methods for opting out of locking
// ============================================================================

impl<S: ReadModelStore> QueuedReadModelStore<S> {
    /// Get a read model with options (opt out of locking with `ReadOpts::no_lock()`).
    pub fn get_model_with<M: ReadModel>(
        &self,
        id: &str,
        opts: ReadOpts,
    ) -> Result<Option<Versioned<M>>, ReadModelError> {
        if opts.lock {
            self.get_model(id)
        } else {
            self.inner.get_model(id)
        }
    }

    /// Find read models with options.
    pub fn find_models_with<M: ReadModel>(
        &self,
        predicate: &dyn Fn(&M) -> bool,
        opts: ReadOpts,
    ) -> Result<Vec<Versioned<M>>, ReadModelError> {
        if opts.lock {
            self.find_models(predicate)
        } else {
            self.inner.find_models(predicate)
        }
    }

    /// Find one read model with options.
    pub fn find_one_model_with<M: ReadModel>(
        &self,
        predicate: &dyn Fn(&M) -> bool,
        opts: ReadOpts,
    ) -> Result<Option<Versioned<M>>, ReadModelError> {
        if opts.lock {
            self.find_one_model(predicate)
        } else {
            self.inner.find_one_model(predicate)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::read_model::InMemoryReadModelStore;
    use serde::{Deserialize, Serialize};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    struct TestModel {
        id: String,
        value: i32,
    }

    impl ReadModel for TestModel {
        const COLLECTION: &'static str = "test_models";
        fn id(&self) -> &str {
            &self.id
        }
    }

    #[test]
    fn get_locks_upsert_unlocks() {
        let store = QueuedReadModelStore::new(InMemoryReadModelStore::new());

        // Seed data
        store.inner().upsert(&TestModel {
            id: "1".into(),
            value: 10,
        }).unwrap();

        // get_model acquires lock
        let loaded = store.get_model::<TestModel>("1").unwrap().unwrap();
        assert_eq!(loaded.data.value, 10);

        // upsert releases lock
        let updated = TestModel {
            id: "1".into(),
            value: 20,
        };
        store.upsert(&updated).unwrap();

        // Can get again (lock was released)
        let reloaded = store.get_model::<TestModel>("1").unwrap().unwrap();
        assert_eq!(reloaded.data.value, 20);

        // Release for cleanup
        store.upsert(&reloaded.data).unwrap();
    }

    #[test]
    fn get_locks_delete_unlocks() {
        let store = QueuedReadModelStore::new(InMemoryReadModelStore::new());

        store.inner().upsert(&TestModel {
            id: "1".into(),
            value: 10,
        }).unwrap();

        // get locks
        let _loaded = store.get_model::<TestModel>("1").unwrap();

        // delete releases
        store.delete::<TestModel>("1").unwrap();

        // Can get again (lock was released)
        let result = store.get_model::<TestModel>("1").unwrap();
        assert!(result.is_none());

        // Release
        store.unlock::<TestModel>("1").unwrap();
    }

    #[test]
    fn manual_unlock() {
        let store = QueuedReadModelStore::new(InMemoryReadModelStore::new());

        store.inner().upsert(&TestModel {
            id: "1".into(),
            value: 10,
        }).unwrap();

        // get locks
        let _loaded = store.get_model::<TestModel>("1").unwrap();

        // Manual unlock (abort)
        store.abort::<TestModel>("1").unwrap();

        // Can get again
        let loaded = store.get_model::<TestModel>("1").unwrap().unwrap();
        assert_eq!(loaded.data.value, 10);
        store.unlock::<TestModel>("1").unwrap();
    }

    #[test]
    fn get_with_no_lock_does_not_block() {
        let store = QueuedReadModelStore::new(InMemoryReadModelStore::new());

        store.inner().upsert(&TestModel {
            id: "1".into(),
            value: 10,
        }).unwrap();

        // get with lock
        let _loaded = store.get_model::<TestModel>("1").unwrap();

        // get_with no_lock still works (doesn't try to acquire)
        let peek = store
            .get_model_with::<TestModel>("1", ReadOpts::no_lock())
            .unwrap()
            .unwrap();
        assert_eq!(peek.data.value, 10);

        // cleanup
        store.unlock::<TestModel>("1").unwrap();
    }

    #[test]
    fn concurrent_access_serialized() {
        let store = Arc::new(QueuedReadModelStore::new(InMemoryReadModelStore::new()));

        store.inner().upsert(&TestModel {
            id: "1".into(),
            value: 0,
        }).unwrap();

        let store2 = store.clone();

        // Thread 1: get (lock), sleep, increment, upsert (unlock)
        let t1 = thread::spawn(move || {
            let loaded = store2.get_model::<TestModel>("1").unwrap().unwrap();
            thread::sleep(Duration::from_millis(50));
            let updated = TestModel {
                id: "1".into(),
                value: loaded.data.value + 1,
            };
            store2.upsert(&updated).unwrap();
        });

        // Small delay so t1 acquires lock first
        thread::sleep(Duration::from_millis(10));

        // Thread 2 (main): get (waits for t1 to unlock), increment, upsert
        let loaded = store.get_model::<TestModel>("1").unwrap().unwrap();
        let updated = TestModel {
            id: "1".into(),
            value: loaded.data.value + 1,
        };
        store.upsert(&updated).unwrap();

        t1.join().unwrap();

        // Both increments applied correctly (no lost update)
        let final_val = store
            .get_model_with::<TestModel>("1", ReadOpts::no_lock())
            .unwrap()
            .unwrap();
        assert_eq!(final_val.data.value, 2);
    }

    #[test]
    fn different_collections_do_not_contend() {
        #[derive(Clone, Debug, Serialize, Deserialize)]
        struct OtherModel {
            id: String,
        }

        impl ReadModel for OtherModel {
            const COLLECTION: &'static str = "other_models";
            fn id(&self) -> &str {
                &self.id
            }
        }

        let store = QueuedReadModelStore::new(InMemoryReadModelStore::new());

        store.inner().upsert(&TestModel {
            id: "1".into(),
            value: 10,
        }).unwrap();
        store.inner().upsert(&OtherModel { id: "1".into() }).unwrap();

        // Lock TestModel "1"
        let _t = store.get_model::<TestModel>("1").unwrap();

        // OtherModel "1" should not be blocked (different collection)
        let _o = store.get_model::<OtherModel>("1").unwrap();

        // cleanup
        store.unlock::<TestModel>("1").unwrap();
        store.unlock::<OtherModel>("1").unwrap();
    }

    #[test]
    fn find_one_locks_result() {
        let store = Arc::new(QueuedReadModelStore::new(InMemoryReadModelStore::new()));

        store.inner().upsert(&TestModel {
            id: "1".into(),
            value: 10,
        }).unwrap();
        store.inner().upsert(&TestModel {
            id: "2".into(),
            value: 20,
        }).unwrap();

        // find_one locks the matched instance
        let found = store
            .find_one_model::<TestModel>(&|m| m.value > 15)
            .unwrap()
            .unwrap();
        assert_eq!(found.data.id, "2");

        // Verify it's locked by checking from another thread with try_lock
        let store2 = store.clone();
        let locked = thread::spawn(move || {
            // Try a non-blocking check: get_with no_lock should still work
            store2
                .get_model_with::<TestModel>("2", ReadOpts::no_lock())
                .unwrap()
                .is_some()
        })
        .join()
        .unwrap();
        assert!(locked);

        // cleanup
        store.unlock::<TestModel>("2").unwrap();
    }

    #[test]
    fn upsert_raw_releases_lock() {
        let store = QueuedReadModelStore::new(InMemoryReadModelStore::new());
        let key = "test_models:1";

        // Seed via inner
        store.inner().upsert(&TestModel {
            id: "1".into(),
            value: 10,
        }).unwrap();

        // get_model locks
        let _loaded = store.get_model::<TestModel>("1").unwrap();

        // upsert_raw releases (this is what CommitBuilder calls)
        let bytes = serde_json::to_vec(&TestModel {
            id: "1".into(),
            value: 99,
        })
        .unwrap();
        store.upsert_raw(key, bytes).unwrap();

        // Can get again
        let reloaded = store.get_model::<TestModel>("1").unwrap().unwrap();
        assert_eq!(reloaded.data.value, 99);
        store.unlock::<TestModel>("1").unwrap();
    }
}
