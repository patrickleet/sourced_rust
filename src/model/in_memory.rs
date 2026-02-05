//! InMemoryModelStore - HashMap-backed model store for testing and development.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use super::{Model, ModelError, ModelStore, Versioned};

/// Internal stored representation of a model.
struct StoredModel {
    bytes: Vec<u8>,
    version: u64,
}

/// In-memory model store backed by a HashMap.
///
/// Storage key is `"TABLE:id"`. Clone-friendly via Arc.
#[derive(Clone)]
pub struct InMemoryModelStore {
    storage: Arc<RwLock<HashMap<String, StoredModel>>>,
}

impl Default for InMemoryModelStore {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryModelStore {
    /// Create a new empty model store.
    pub fn new() -> Self {
        Self {
            storage: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    fn make_key(table: &str, id: &str) -> String {
        format!("{}:{}", table, id)
    }

    /// Save a raw model entry (used by CommitBuilder for type-erased writes).
    pub(crate) fn save_raw(&self, key: &str, bytes: Vec<u8>) -> Result<u64, ModelError> {
        let mut storage = self
            .storage
            .write()
            .map_err(|_| ModelError::Storage("lock poisoned".into()))?;

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

impl ModelStore for InMemoryModelStore {
    fn get_model<M: Model>(&self, id: &str) -> Result<Option<Versioned<M>>, ModelError> {
        let key = Self::make_key(M::COLLECTION, id);
        let storage = self
            .storage
            .read()
            .map_err(|_| ModelError::Storage("lock poisoned".into()))?;

        match storage.get(&key) {
            Some(stored) => {
                let data: M = serde_json::from_slice(&stored.bytes)
                    .map_err(|e| ModelError::Serde(e.to_string()))?;
                Ok(Some(Versioned {
                    data,
                    version: stored.version,
                }))
            }
            None => Ok(None),
        }
    }

    fn save_model<M: Model>(&self, model: &M) -> Result<Versioned<M>, ModelError> {
        let key = Self::make_key(M::COLLECTION, model.id());
        let bytes =
            serde_json::to_vec(model).map_err(|e| ModelError::Serde(e.to_string()))?;

        let mut storage = self
            .storage
            .write()
            .map_err(|_| ModelError::Storage("lock poisoned".into()))?;

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

    fn insert_model<M: Model>(&self, model: &M) -> Result<Versioned<M>, ModelError> {
        let key = Self::make_key(M::COLLECTION, model.id());
        let bytes =
            serde_json::to_vec(model).map_err(|e| ModelError::Serde(e.to_string()))?;

        let mut storage = self
            .storage
            .write()
            .map_err(|_| ModelError::Storage("lock poisoned".into()))?;

        if storage.contains_key(&key) {
            return Err(ModelError::ConcurrencyConflict {
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

    fn update_model<M: Model>(
        &self,
        model: &M,
        expected_version: u64,
    ) -> Result<Versioned<M>, ModelError> {
        let key = Self::make_key(M::COLLECTION, model.id());
        let bytes =
            serde_json::to_vec(model).map_err(|e| ModelError::Serde(e.to_string()))?;

        let mut storage = self
            .storage
            .write()
            .map_err(|_| ModelError::Storage("lock poisoned".into()))?;

        let actual_version = storage
            .get(&key)
            .map(|s| s.version)
            .ok_or_else(|| ModelError::NotFound {
                collection: M::COLLECTION.to_string(),
                id: model.id().to_string(),
            })?;

        if actual_version != expected_version {
            return Err(ModelError::ConcurrencyConflict {
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

    fn delete_model<M: Model>(&self, id: &str) -> Result<bool, ModelError> {
        let key = Self::make_key(M::COLLECTION, id);
        let mut storage = self
            .storage
            .write()
            .map_err(|_| ModelError::Storage("lock poisoned".into()))?;

        Ok(storage.remove(&key).is_some())
    }

    fn find_models<M: Model>(
        &self,
        predicate: &dyn Fn(&M) -> bool,
    ) -> Result<Vec<Versioned<M>>, ModelError> {
        let storage = self
            .storage
            .read()
            .map_err(|_| ModelError::Storage("lock poisoned".into()))?;

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

    fn save_model_raw(&self, key: &str, bytes: Vec<u8>) -> Result<(), ModelError> {
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

    impl Model for TestModel {
        const COLLECTION: &'static str = "test_models";
        fn id(&self) -> &str {
            &self.id
        }
    }

    #[test]
    fn save_and_get() {
        let store = InMemoryModelStore::new();
        let model = TestModel {
            id: "1".into(),
            value: 42,
        };

        let saved = store.save_model(&model).unwrap();
        assert_eq!(saved.version, 1);
        assert_eq!(saved.data.value, 42);

        let loaded = store.get_model::<TestModel>("1").unwrap().unwrap();
        assert_eq!(loaded.version, 1);
        assert_eq!(loaded.data.value, 42);
    }

    #[test]
    fn save_increments_version() {
        let store = InMemoryModelStore::new();
        let model = TestModel {
            id: "1".into(),
            value: 1,
        };

        store.save_model(&model).unwrap();
        let updated = TestModel {
            id: "1".into(),
            value: 2,
        };
        let saved = store.save_model(&updated).unwrap();
        assert_eq!(saved.version, 2);
    }

    #[test]
    fn get_missing_returns_none() {
        let store = InMemoryModelStore::new();
        let result = store.get_model::<TestModel>("missing").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn insert_fails_on_existing() {
        let store = InMemoryModelStore::new();
        let model = TestModel {
            id: "1".into(),
            value: 1,
        };

        store.insert_model(&model).unwrap();
        let err = store.insert_model(&model).unwrap_err();
        assert!(matches!(err, ModelError::ConcurrencyConflict { .. }));
    }

    #[test]
    fn update_with_correct_version() {
        let store = InMemoryModelStore::new();
        let model = TestModel {
            id: "1".into(),
            value: 1,
        };

        store.save_model(&model).unwrap();

        let updated = TestModel {
            id: "1".into(),
            value: 2,
        };
        let result = store.update_model(&updated, 1).unwrap();
        assert_eq!(result.version, 2);
        assert_eq!(result.data.value, 2);
    }

    #[test]
    fn update_with_wrong_version_fails() {
        let store = InMemoryModelStore::new();
        let model = TestModel {
            id: "1".into(),
            value: 1,
        };

        store.save_model(&model).unwrap();

        let updated = TestModel {
            id: "1".into(),
            value: 2,
        };
        let err = store.update_model(&updated, 99).unwrap_err();
        assert!(matches!(err, ModelError::ConcurrencyConflict { .. }));
    }

    #[test]
    fn delete_existing() {
        let store = InMemoryModelStore::new();
        let model = TestModel {
            id: "1".into(),
            value: 1,
        };

        store.save_model(&model).unwrap();
        assert!(store.delete_model::<TestModel>("1").unwrap());
        assert!(store.get_model::<TestModel>("1").unwrap().is_none());
    }

    #[test]
    fn delete_missing_returns_false() {
        let store = InMemoryModelStore::new();
        assert!(!store.delete_model::<TestModel>("missing").unwrap());
    }

    #[test]
    fn find_models_with_predicate() {
        let store = InMemoryModelStore::new();

        store
            .save_model(&TestModel {
                id: "1".into(),
                value: 10,
            })
            .unwrap();
        store
            .save_model(&TestModel {
                id: "2".into(),
                value: 20,
            })
            .unwrap();
        store
            .save_model(&TestModel {
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
    fn clone_shares_storage() {
        let store = InMemoryModelStore::new();
        let clone = store.clone();

        store
            .save_model(&TestModel {
                id: "1".into(),
                value: 42,
            })
            .unwrap();

        let loaded = clone.get_model::<TestModel>("1").unwrap().unwrap();
        assert_eq!(loaded.data.value, 42);
    }
}
