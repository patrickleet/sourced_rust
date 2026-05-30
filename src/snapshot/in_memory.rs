#![expect(
    clippy::manual_async_fn,
    reason = "async trait impls return impl Future + Send to preserve public Send bounds"
)]

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, RwLock};

use crate::repository::{AsyncSnapshotStore, RepositoryError, StreamIdentity};

use super::store::SnapshotRecord;

/// In-memory snapshot store backed by `Arc<RwLock<HashMap>>`.
///
/// Clone-friendly (cloning shares the same underlying storage).
/// Follows the same pattern as `InMemoryReadModelStore`.
#[derive(Clone)]
pub struct InMemorySnapshotStore {
    pub(crate) storage: Arc<RwLock<HashMap<String, SnapshotRecord>>>,
}

impl Default for InMemorySnapshotStore {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemorySnapshotStore {
    pub fn new() -> Self {
        Self {
            storage: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl AsyncSnapshotStore for InMemorySnapshotStore {
    fn get_snapshot_async<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<Option<SnapshotRecord>, RepositoryError>> + Send + 'a {
        async move {
            let storage = self
                .storage
                .read()
                .map_err(|_| RepositoryError::LockPoisoned("async snapshot read"))?;
            Ok(storage.get(&identity.storage_key()).cloned())
        }
    }

    fn save_snapshot_async<'a>(
        &'a self,
        identity: &'a StreamIdentity,
        record: SnapshotRecord,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        async move {
            record.validate_for_identity(identity)?;
            let mut storage = self
                .storage
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("async snapshot write"))?;
            storage.insert(identity.storage_key(), record);
            Ok(())
        }
    }

    fn delete_snapshot_async<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<bool, RepositoryError>> + Send + 'a {
        async move {
            let mut storage = self
                .storage
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("async snapshot write"))?;
            Ok(storage.remove(&identity.storage_key()).is_some())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block_on<F: Future>(future: F) -> F::Output {
        use std::ptr;
        use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
        const VTABLE: RawWakerVTable = RawWakerVTable::new(
            |_| RawWaker::new(ptr::null(), &VTABLE),
            |_| {},
            |_| {},
            |_| {},
        );
        let waker = unsafe { Waker::from_raw(RawWaker::new(ptr::null(), &VTABLE)) };
        let mut cx = Context::from_waker(&waker);
        let mut future = std::pin::pin!(future);
        loop {
            if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
                return output;
            }
        }
    }

    fn identity(id: &str) -> StreamIdentity {
        StreamIdentity::new("test.aggregate", id).unwrap()
    }

    #[test]
    fn save_and_get() {
        let store = InMemorySnapshotStore::new();
        let record = SnapshotRecord::new(
            "test.aggregate",
            "agg-1",
            5,
            "TestSnapshot",
            1,
            vec![1, 2, 3],
        );
        block_on(store.save_snapshot_async(&identity("agg-1"), record)).unwrap();

        let loaded = block_on(store.get_snapshot_async(&identity("agg-1")))
            .unwrap()
            .unwrap();
        assert_eq!(loaded.version, 5);
        assert_eq!(loaded.payload, vec![1, 2, 3]);
        assert_eq!(loaded.snapshot_type, "TestSnapshot");
    }

    #[test]
    fn get_missing_returns_none() {
        let store = InMemorySnapshotStore::new();
        assert!(block_on(store.get_snapshot_async(&identity("missing")))
            .unwrap()
            .is_none());
    }

    #[test]
    fn save_overwrites() {
        let store = InMemorySnapshotStore::new();
        block_on(store.save_snapshot_async(
            &identity("agg-1"),
            SnapshotRecord::new("test.aggregate", "agg-1", 1, "TestSnapshot", 1, vec![1]),
        ))
        .unwrap();
        block_on(store.save_snapshot_async(
            &identity("agg-1"),
            SnapshotRecord::new("test.aggregate", "agg-1", 5, "TestSnapshot", 1, vec![5]),
        ))
        .unwrap();

        let loaded = block_on(store.get_snapshot_async(&identity("agg-1")))
            .unwrap()
            .unwrap();
        assert_eq!(loaded.version, 5);
        assert_eq!(loaded.payload, vec![5]);
    }

    #[test]
    fn delete_existing() {
        let store = InMemorySnapshotStore::new();
        block_on(store.save_snapshot_async(
            &identity("agg-1"),
            SnapshotRecord::new("test.aggregate", "agg-1", 1, "TestSnapshot", 1, vec![1]),
        ))
        .unwrap();
        assert!(block_on(store.delete_snapshot_async(&identity("agg-1"))).unwrap());
        assert!(block_on(store.get_snapshot_async(&identity("agg-1")))
            .unwrap()
            .is_none());
    }

    #[test]
    fn delete_missing_returns_false() {
        let store = InMemorySnapshotStore::new();
        assert!(!block_on(store.delete_snapshot_async(&identity("missing"))).unwrap());
    }

    #[test]
    fn clone_shares_storage() {
        let store = InMemorySnapshotStore::new();
        let clone = store.clone();
        block_on(store.save_snapshot_async(
            &identity("agg-1"),
            SnapshotRecord::new("test.aggregate", "agg-1", 3, "TestSnapshot", 1, vec![3]),
        ))
        .unwrap();

        let loaded = block_on(clone.get_snapshot_async(&identity("agg-1")))
            .unwrap()
            .unwrap();
        assert_eq!(loaded.version, 3);
    }
}
