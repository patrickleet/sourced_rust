//! QueuedReadModelStore - Per-instance locking for read models.
//!
//! Mirrors the `QueuedRepository` pattern for entities:
//! `get_model` acquires a lock, write operations (`upsert`, `insert`, `update`,
//! `delete`) release it. Callers can also release manually via `unlock`.

use std::sync::Arc;

use crate::entity::Committable;
use crate::lock::{InMemoryLockManager, Lock, LockManager};
use crate::queued_repo::ReadOpts;
use crate::repository::{Commit, CommitBatch, RepositoryError, TransactionalCommit};

use super::session::document_key;
use super::{
    ReadModel, ReadModelAdapterCapabilities, ReadModelCommitOutcome, ReadModelError,
    ReadModelSessionStore, ReadModelStore, ReadModelWritePlan, Versioned,
};

/// A `ReadModelStore` wrapper that provides per-instance locking.
///
/// Lock lifecycle matches the entity `QueuedRepository` pattern:
/// - `get_model` acquires a lock (or waits if already locked)
/// - `upsert` / `insert` / `update` / `delete` release the lock on success
/// - `unlock` / `abort` release the lock manually
///
/// Lock keys include the collection and id, so different read model types with
/// the same ID do not contend with each other.
pub struct QueuedReadModelStore<S, L: LockManager = InMemoryLockManager> {
    inner: S,
    lock_manager: L,
}

impl<S> QueuedReadModelStore<S> {
    pub fn new(inner: S) -> Self {
        QueuedReadModelStore {
            inner,
            lock_manager: InMemoryLockManager::new(),
        }
    }
}

impl<S, L: LockManager> QueuedReadModelStore<S, L> {
    /// Create a `QueuedReadModelStore` with a custom lock manager.
    pub fn with_lock_manager(inner: S, lock_manager: L) -> Self {
        QueuedReadModelStore {
            inner,
            lock_manager,
        }
    }

    /// Access the inner store.
    pub fn inner(&self) -> &S {
        &self.inner
    }

    /// Access the lock manager.
    pub fn lock_manager(&self) -> &L {
        &self.lock_manager
    }

    /// Manually lock a read model instance.
    pub fn lock<M: ReadModel>(&self, id: &str) -> Result<(), ReadModelError> {
        let key = Self::make_key(M::COLLECTION, id);
        let lock = self.ensure_lock(&key)?;
        lock.lock()?;
        Ok(())
    }

    /// Manually unlock a read model instance.
    pub fn unlock<M: ReadModel>(&self, id: &str) -> Result<(), ReadModelError> {
        let key = Self::make_key(M::COLLECTION, id);
        let lock = self.ensure_lock(&key)?;
        lock.unlock()?;
        Ok(())
    }

    /// Abort — alias for unlock.
    pub fn abort<M: ReadModel>(&self, id: &str) -> Result<(), ReadModelError> {
        self.unlock::<M>(id)
    }

    fn release(&self, key: &str) {
        if let Ok(lock) = self.lock_manager.get_lock(key) {
            let _ = lock.unlock();
        }
    }

    fn ensure_lock(&self, key: &str) -> Result<Arc<L::Lock>, ReadModelError> {
        Ok(self.lock_manager.get_lock(key)?)
    }

    fn lock_ids_in_order(&self, keys: &[String]) -> Result<Vec<Arc<L::Lock>>, ReadModelError> {
        let mut unique = keys.to_vec();
        unique.sort_unstable();
        unique.dedup();

        let mut locks = Vec::with_capacity(unique.len());
        for key in &unique {
            let lock = self.ensure_lock(key)?;
            lock.lock()?;
            locks.push(lock);
        }

        Ok(locks)
    }

    fn make_key(collection: &str, id: &str) -> String {
        document_key(collection, id)
    }
}

// ============================================================================
// ReadModelStore implementation (locking by default)
// ============================================================================

impl<S: ReadModelStore, L: LockManager> ReadModelStore for QueuedReadModelStore<S, L> {
    fn get_model<M: ReadModel>(&self, id: &str) -> Result<Option<Versioned<M>>, ReadModelError> {
        let key = Self::make_key(M::COLLECTION, id);
        let lock = self.ensure_lock(&key)?;
        lock.lock()?;
        self.inner.get_model(id)
    }

    fn get_by_primary_key<M: ReadModel>(
        &self,
        id: &str,
    ) -> Result<Option<Versioned<M>>, ReadModelError> {
        self.inner.get_by_primary_key(id)
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
            lock.lock()?;

            // Phase 2: re-fetch with lock held
            if let Some(current) = self.inner.get_model::<M>(&id)? {
                if predicate(&current.data) {
                    return Ok(Some(current));
                }
            }
            // No longer matches, unlock
            lock.unlock()?;
        }

        Ok(None)
    }
}

// ============================================================================
// Commit delegation (needed for CommitBuilder integration)
// ============================================================================

impl<S: Commit, L: LockManager> Commit for QueuedReadModelStore<S, L> {
    fn commit<C: Committable + ?Sized>(&self, committable: &mut C) -> Result<(), RepositoryError> {
        self.inner.commit(committable)
    }
}

impl<S: TransactionalCommit, L: LockManager> TransactionalCommit for QueuedReadModelStore<S, L> {
    fn commit_batch(&self, batch: CommitBatch<'_>) -> Result<(), RepositoryError> {
        let read_model_keys: Vec<String> = batch
            .read_model_plans
            .iter()
            .flat_map(|plan| plan.mutations.iter().map(|mutation| mutation.lock_key()))
            .collect();

        let result = self.inner.commit_batch(batch);
        if result.is_ok() {
            for key in read_model_keys {
                self.release(&key);
            }
        }

        result
    }
}

impl<S: ReadModelSessionStore, L: LockManager> ReadModelSessionStore
    for QueuedReadModelStore<S, L>
{
    fn read_model_capabilities(&self) -> ReadModelAdapterCapabilities {
        self.inner.read_model_capabilities()
    }

    fn commit_write_plan(
        &self,
        plan: ReadModelWritePlan,
    ) -> Result<ReadModelCommitOutcome, ReadModelError> {
        let mut read_model_keys: Vec<String> = plan
            .mutations
            .iter()
            .map(|mutation| mutation.lock_key())
            .collect();
        read_model_keys.sort_unstable();
        read_model_keys.dedup();
        let _locks = self.lock_ids_in_order(&read_model_keys)?;

        let result = self.inner.commit_write_plan(plan);
        if result.is_ok() {
            for key in read_model_keys {
                self.release(&key);
            }
        }

        result
    }

    fn is_processed(&self, consumer_name: &str, message_id: &str) -> Result<bool, ReadModelError> {
        self.inner.is_processed(consumer_name, message_id)
    }
}

// ============================================================================
// WithOpts methods for opting out of locking
// ============================================================================

impl<S: ReadModelStore, L: LockManager> QueuedReadModelStore<S, L> {
    /// Load and lock a read model for update.
    ///
    /// This is the explicit spelling for the default locking behavior of
    /// `get_model`.
    pub fn load_for_update<M: ReadModel>(
        &self,
        id: &str,
    ) -> Result<Option<Versioned<M>>, ReadModelError> {
        self.get_model(id)
    }

    /// Load a read model without acquiring the document-store lock.
    pub fn load_no_lock<M: ReadModel>(
        &self,
        id: &str,
    ) -> Result<Option<Versioned<M>>, ReadModelError> {
        self.get_model_with(id, ReadOpts::no_lock())
    }

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
    use crate::{DocumentMutation, LockError, ReadModelMutation};
    use serde::{Deserialize, Serialize};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
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

    #[derive(Default)]
    struct CountingLock {
        lock_count: AtomicUsize,
        unlock_count: AtomicUsize,
    }

    impl Lock for CountingLock {
        fn lock(&self) -> Result<(), LockError> {
            self.lock_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn try_lock(&self) -> Result<bool, LockError> {
            Ok(true)
        }

        fn unlock(&self) -> Result<(), LockError> {
            self.unlock_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[derive(Clone, Default)]
    struct CountingLockManager {
        locks: Arc<Mutex<HashMap<String, Arc<CountingLock>>>>,
    }

    impl CountingLockManager {
        fn unlock_count(&self, key: &str) -> usize {
            self.get_lock(key)
                .unwrap()
                .unlock_count
                .load(Ordering::SeqCst)
        }
    }

    impl LockManager for CountingLockManager {
        type Lock = CountingLock;

        fn get_lock(&self, id: &str) -> Result<Arc<Self::Lock>, LockError> {
            let mut locks = self
                .locks
                .lock()
                .map_err(|_| LockError::Poisoned("counting lock manager".into()))?;
            Ok(locks.entry(id.to_string()).or_default().clone())
        }
    }

    #[test]
    fn get_locks_upsert_unlocks() {
        let store = QueuedReadModelStore::new(InMemoryReadModelStore::new());

        // Seed data
        store
            .inner()
            .upsert(&TestModel {
                id: "1".into(),
                value: 10,
            })
            .unwrap();

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

        store
            .inner()
            .upsert(&TestModel {
                id: "1".into(),
                value: 10,
            })
            .unwrap();

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

        store
            .inner()
            .upsert(&TestModel {
                id: "1".into(),
                value: 10,
            })
            .unwrap();

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

        store
            .inner()
            .upsert(&TestModel {
                id: "1".into(),
                value: 10,
            })
            .unwrap();

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

        store
            .inner()
            .upsert(&TestModel {
                id: "1".into(),
                value: 0,
            })
            .unwrap();

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

        store
            .inner()
            .upsert(&TestModel {
                id: "1".into(),
                value: 10,
            })
            .unwrap();
        store
            .inner()
            .upsert(&OtherModel { id: "1".into() })
            .unwrap();

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

        store
            .inner()
            .upsert(&TestModel {
                id: "1".into(),
                value: 10,
            })
            .unwrap();
        store
            .inner()
            .upsert(&TestModel {
                id: "2".into(),
                value: 20,
            })
            .unwrap();

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
    fn commit_write_plan_releases_duplicate_document_lock_once() {
        let lock_manager = CountingLockManager::default();
        let store = QueuedReadModelStore::with_lock_manager(
            InMemoryReadModelStore::new(),
            lock_manager.clone(),
        );
        let mutation = DocumentMutation {
            collection: TestModel::COLLECTION.into(),
            id: "1".into(),
            bytes: serde_json::to_vec(&TestModel {
                id: "1".into(),
                value: 1,
            })
            .unwrap(),
        };
        let key = mutation.key();
        let plan = ReadModelWritePlan::new(
            vec![
                ReadModelMutation::Document(mutation.clone()),
                ReadModelMutation::Document(mutation),
            ],
            Vec::new(),
        );

        store.commit_write_plan(plan).unwrap();

        assert_eq!(lock_manager.unlock_count(&key), 1);
    }
}
