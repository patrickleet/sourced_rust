use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::repository::RepositoryError;

use super::store::{SnapshotRecord, SnapshotStore};

/// In-memory snapshot store backed by `Arc<RwLock<HashMap>>`.
///
/// Clone-friendly (cloning shares the same underlying storage).
/// Follows the same pattern as `InMemoryReadModelStore`.
#[derive(Clone)]
pub struct InMemorySnapshotStore {
    storage: Arc<RwLock<HashMap<String, SnapshotRecord>>>,
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

impl SnapshotStore for InMemorySnapshotStore {
    fn get_snapshot(&self, id: &str) -> Result<Option<SnapshotRecord>, RepositoryError> {
        let storage = self
            .storage
            .read()
            .map_err(|_| RepositoryError::LockPoisoned("snapshot read"))?;
        Ok(storage.get(id).cloned())
    }

    fn save_snapshot(&self, record: SnapshotRecord) -> Result<(), RepositoryError> {
        let mut storage = self
            .storage
            .write()
            .map_err(|_| RepositoryError::LockPoisoned("snapshot write"))?;
        storage.insert(record.aggregate_id.clone(), record);
        Ok(())
    }

    fn delete_snapshot(&self, id: &str) -> Result<bool, RepositoryError> {
        let mut storage = self
            .storage
            .write()
            .map_err(|_| RepositoryError::LockPoisoned("snapshot write"))?;
        Ok(storage.remove(id).is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_and_get() {
        let store = InMemorySnapshotStore::new();
        let record = SnapshotRecord {
            aggregate_id: "agg-1".into(),
            version: 5,
            data: vec![1, 2, 3],
        };
        store.save_snapshot(record).unwrap();

        let loaded = store.get_snapshot("agg-1").unwrap().unwrap();
        assert_eq!(loaded.version, 5);
        assert_eq!(loaded.data, vec![1, 2, 3]);
    }

    #[test]
    fn get_missing_returns_none() {
        let store = InMemorySnapshotStore::new();
        assert!(store.get_snapshot("missing").unwrap().is_none());
    }

    #[test]
    fn save_overwrites() {
        let store = InMemorySnapshotStore::new();
        store
            .save_snapshot(SnapshotRecord {
                aggregate_id: "agg-1".into(),
                version: 1,
                data: vec![1],
            })
            .unwrap();
        store
            .save_snapshot(SnapshotRecord {
                aggregate_id: "agg-1".into(),
                version: 5,
                data: vec![5],
            })
            .unwrap();

        let loaded = store.get_snapshot("agg-1").unwrap().unwrap();
        assert_eq!(loaded.version, 5);
        assert_eq!(loaded.data, vec![5]);
    }

    #[test]
    fn delete_existing() {
        let store = InMemorySnapshotStore::new();
        store
            .save_snapshot(SnapshotRecord {
                aggregate_id: "agg-1".into(),
                version: 1,
                data: vec![1],
            })
            .unwrap();
        assert!(store.delete_snapshot("agg-1").unwrap());
        assert!(store.get_snapshot("agg-1").unwrap().is_none());
    }

    #[test]
    fn delete_missing_returns_false() {
        let store = InMemorySnapshotStore::new();
        assert!(!store.delete_snapshot("missing").unwrap());
    }

    #[test]
    fn clone_shares_storage() {
        let store = InMemorySnapshotStore::new();
        let clone = store.clone();
        store
            .save_snapshot(SnapshotRecord {
                aggregate_id: "agg-1".into(),
                version: 3,
                data: vec![3],
            })
            .unwrap();

        let loaded = clone.get_snapshot("agg-1").unwrap().unwrap();
        assert_eq!(loaded.version, 3);
    }
}
