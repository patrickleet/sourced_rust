//! InMemoryReadModelStore - HashMap-backed read model store for testing and development.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use super::{ReadModel, ReadModelError, ReadModelStore, Versioned};

/// Internal stored representation of a read model.
struct StoredModel {
    bytes: Vec<u8>,
    version: u64,
}

/// In-memory read model store backed by a HashMap.
///
/// Storage key is `"TABLE:id"`. Clone-friendly via Arc.
#[derive(Clone)]
pub struct InMemoryReadModelStore {
    storage: Arc<RwLock<HashMap<String, StoredModel>>>,
}

impl Default for InMemoryReadModelStore {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryReadModelStore {
    /// Create a new empty read model store.
    pub fn new() -> Self {
        Self {
            storage: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    fn make_key(table: &str, id: &str) -> String {
        format!("{}:{}", table, id)
    }

    /// Save a raw read model entry (used by CommitBuilder for type-erased writes).
    pub(crate) fn save_raw(&self, key: &str, bytes: Vec<u8>) -> Result<u64, ReadModelError> {
        let mut storage = self
            .storage
            .write()
            .map_err(|_| ReadModelError::Storage("lock poisoned".into()))?;

        let new_version = storage
            .get(key)
            .map(|s| s.version + 1)
            .unwrap_or(1);

        storage.insert(
            key.to_string(),
            StoredModel {
                bytes,
                version: new_version,
            },
        );

        Ok(new_version)
    }
}

impl ReadModelStore for InMemoryReadModelStore {
    fn get_model<M: ReadModel>(&self, id: &str) -> Result<Option<Versioned<M>>, ReadModelError> {
        let key = Self::make_key(M::COLLECTION, id);
        let storage = self
            .storage
            .read()
            .map_err(|_| ReadModelError::Storage("lock poisoned".into()))?;

        match storage.get(&key) {
            Some(stored) => {
                let data: M = serde_json::from_slice(&stored.bytes)
                    .map_err(|e| ReadModelError::Serde(e.to_string()))?;
                Ok(Some(Versioned {
                    data,
                    version: stored.version,
                }))
            }
            None => Ok(None),
        }
    }

    fn upsert<M: ReadModel>(&self, model: &M) -> Result<Versioned<M>, ReadModelError> {
        let key = Self::make_key(M::COLLECTION, model.id());
        let bytes =
            serde_json::to_vec(model).map_err(|e| ReadModelError::Serde(e.to_string()))?;

        let mut storage = self
            .storage
            .write()
            .map_err(|_| ReadModelError::Storage("lock poisoned".into()))?;

        let new_version = storage.get(&key).map(|s| s.version + 1).unwrap_or(1);

        storage.insert(
            key,
            StoredModel {
                bytes,
                version: new_version,
            },
        );

        Ok(Versioned {
            data: model.clone(),
            version: new_version,
        })
    }

    fn insert<M: ReadModel>(&self, model: &M) -> Result<Versioned<M>, ReadModelError> {
        let key = Self::make_key(M::COLLECTION, model.id());
        let bytes =
            serde_json::to_vec(model).map_err(|e| ReadModelError::Serde(e.to_string()))?;

        let mut storage = self
            .storage
            .write()
            .map_err(|_| ReadModelError::Storage("lock poisoned".into()))?;

        if storage.contains_key(&key) {
            return Err(ReadModelError::ConcurrencyConflict {
                collection: M::COLLECTION.to_string(),
                id: model.id().to_string(),
                expected: 0,
                actual: storage[&key].version,
            });
        }

        storage.insert(
            key,
            StoredModel {
                bytes,
                version: 1,
            },
        );

        Ok(Versioned {
            data: model.clone(),
            version: 1,
        })
    }

    fn update<M: ReadModel>(
        &self,
        model: &M,
        expected_version: u64,
    ) -> Result<Versioned<M>, ReadModelError> {
        let key = Self::make_key(M::COLLECTION, model.id());
        let bytes =
            serde_json::to_vec(model).map_err(|e| ReadModelError::Serde(e.to_string()))?;

        let mut storage = self
            .storage
            .write()
            .map_err(|_| ReadModelError::Storage("lock poisoned".into()))?;

        let actual_version = storage
            .get(&key)
            .map(|s| s.version)
            .ok_or_else(|| ReadModelError::NotFound {
                collection: M::COLLECTION.to_string(),
                id: model.id().to_string(),
            })?;

        if actual_version != expected_version {
            return Err(ReadModelError::ConcurrencyConflict {
                collection: M::COLLECTION.to_string(),
                id: model.id().to_string(),
                expected: expected_version,
                actual: actual_version,
            });
        }

        let new_version = actual_version + 1;
        storage.insert(
            key,
            StoredModel {
                bytes,
                version: new_version,
            },
        );

        Ok(Versioned {
            data: model.clone(),
            version: new_version,
        })
    }

    fn delete<M: ReadModel>(&self, id: &str) -> Result<bool, ReadModelError> {
        let key = Self::make_key(M::COLLECTION, id);
        let mut storage = self
            .storage
            .write()
            .map_err(|_| ReadModelError::Storage("lock poisoned".into()))?;

        Ok(storage.remove(&key).is_some())
    }

    fn find_models<M: ReadModel>(
        &self,
        predicate: &dyn Fn(&M) -> bool,
    ) -> Result<Vec<Versioned<M>>, ReadModelError> {
        let storage = self
            .storage
            .read()
            .map_err(|_| ReadModelError::Storage("lock poisoned".into()))?;

        let prefix = format!("{}:", M::COLLECTION);
        let mut results = Vec::new();

        for (key, stored) in storage.iter() {
            if key.starts_with(&prefix) {
                if let Ok(data) = serde_json::from_slice::<M>(&stored.bytes) {
                    if predicate(&data) {
                        results.push(Versioned {
                            data,
                            version: stored.version,
                        });
                    }
                }
            }
        }

        Ok(results)
    }

    fn find_one_model<M: ReadModel>(
        &self,
        predicate: &dyn Fn(&M) -> bool,
    ) -> Result<Option<Versioned<M>>, ReadModelError> {
        let storage = self
            .storage
            .read()
            .map_err(|_| ReadModelError::Storage("lock poisoned".into()))?;

        let prefix = format!("{}:", M::COLLECTION);

        for (key, stored) in storage.iter() {
            if key.starts_with(&prefix) {
                if let Ok(data) = serde_json::from_slice::<M>(&stored.bytes) {
                    if predicate(&data) {
                        return Ok(Some(Versioned {
                            data,
                            version: stored.version,
                        }));
                    }
                }
            }
        }

        Ok(None)
    }

    fn upsert_raw(&self, key: &str, bytes: Vec<u8>) -> Result<(), ReadModelError> {
        self.save_raw(key, bytes)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

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
    fn upsert_and_get() {
        let store = InMemoryReadModelStore::new();
        let model = TestModel {
            id: "1".into(),
            value: 42,
        };

        let saved = store.upsert(&model).unwrap();
        assert_eq!(saved.version, 1);
        assert_eq!(saved.data.value, 42);

        let loaded = store.get_model::<TestModel>("1").unwrap().unwrap();
        assert_eq!(loaded.version, 1);
        assert_eq!(loaded.data.value, 42);
    }

    #[test]
    fn upsert_increments_version() {
        let store = InMemoryReadModelStore::new();
        let model = TestModel {
            id: "1".into(),
            value: 1,
        };

        store.upsert(&model).unwrap();
        let updated = TestModel {
            id: "1".into(),
            value: 2,
        };
        let saved = store.upsert(&updated).unwrap();
        assert_eq!(saved.version, 2);
    }

    #[test]
    fn get_missing_returns_none() {
        let store = InMemoryReadModelStore::new();
        let result = store.get_model::<TestModel>("missing").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn insert_fails_on_existing() {
        let store = InMemoryReadModelStore::new();
        let model = TestModel {
            id: "1".into(),
            value: 1,
        };

        store.insert(&model).unwrap();
        let err = store.insert(&model).unwrap_err();
        assert!(matches!(err, ReadModelError::ConcurrencyConflict { .. }));
    }

    #[test]
    fn update_with_correct_version() {
        let store = InMemoryReadModelStore::new();
        let model = TestModel {
            id: "1".into(),
            value: 1,
        };

        store.upsert(&model).unwrap();

        let updated = TestModel {
            id: "1".into(),
            value: 2,
        };
        let result = store.update(&updated, 1).unwrap();
        assert_eq!(result.version, 2);
        assert_eq!(result.data.value, 2);
    }

    #[test]
    fn update_with_wrong_version_fails() {
        let store = InMemoryReadModelStore::new();
        let model = TestModel {
            id: "1".into(),
            value: 1,
        };

        store.upsert(&model).unwrap();

        let updated = TestModel {
            id: "1".into(),
            value: 2,
        };
        let err = store.update(&updated, 99).unwrap_err();
        assert!(matches!(err, ReadModelError::ConcurrencyConflict { .. }));
    }

    #[test]
    fn delete_existing() {
        let store = InMemoryReadModelStore::new();
        let model = TestModel {
            id: "1".into(),
            value: 1,
        };

        store.upsert(&model).unwrap();
        assert!(store.delete::<TestModel>("1").unwrap());
        assert!(store.get_model::<TestModel>("1").unwrap().is_none());
    }

    #[test]
    fn delete_missing_returns_false() {
        let store = InMemoryReadModelStore::new();
        assert!(!store.delete::<TestModel>("missing").unwrap());
    }

    #[test]
    fn find_with_predicate() {
        let store = InMemoryReadModelStore::new();

        store
            .upsert(&TestModel {
                id: "1".into(),
                value: 10,
            })
            .unwrap();
        store
            .upsert(&TestModel {
                id: "2".into(),
                value: 20,
            })
            .unwrap();
        store
            .upsert(&TestModel {
                id: "3".into(),
                value: 5,
            })
            .unwrap();

        let results = store
            .find_models::<TestModel>(&|m| m.value > 8)
            .unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn find_one_with_predicate() {
        let store = InMemoryReadModelStore::new();

        store
            .upsert(&TestModel {
                id: "1".into(),
                value: 10,
            })
            .unwrap();
        store
            .upsert(&TestModel {
                id: "2".into(),
                value: 20,
            })
            .unwrap();

        let result = store
            .find_one_model::<TestModel>(&|m| m.value > 15)
            .unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().data.value, 20);

        let none = store
            .find_one_model::<TestModel>(&|m| m.value > 100)
            .unwrap();
        assert!(none.is_none());
    }

    #[test]
    fn clone_shares_storage() {
        let store = InMemoryReadModelStore::new();
        let clone = store.clone();

        store
            .upsert(&TestModel {
                id: "1".into(),
                value: 42,
            })
            .unwrap();

        let loaded = clone.get_model::<TestModel>("1").unwrap().unwrap();
        assert_eq!(loaded.data.value, 42);
    }
}
