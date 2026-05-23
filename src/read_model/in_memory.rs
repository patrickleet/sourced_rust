//! InMemoryReadModelStore - HashMap-backed read model store for testing and development.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use super::{
    ProcessedMessageMark, ReadModel, ReadModelAdapterCapabilities, ReadModelCommitOutcome,
    ReadModelError, ReadModelMutation, ReadModelSessionStore, ReadModelStore, ReadModelWritePlan,
    Versioned,
};

/// Internal stored representation of a read model.
#[derive(Clone)]
pub(crate) struct StoredModel {
    pub(crate) bytes: Vec<u8>,
    pub(crate) version: u64,
}

pub(crate) type ProcessedMessageSet = HashSet<(String, String)>;

pub(crate) const INITIAL_MODEL_VERSION: u64 = 1;

/// Return the next optimistic version for a read model row.
///
/// Missing rows start at version 1; existing rows must increment without
/// overflowing so release builds cannot wrap back to 0.
pub(crate) fn next_model_version(
    key: &str,
    current_version: Option<u64>,
) -> Result<u64, ReadModelError> {
    match current_version {
        Some(version) => version.checked_add(1).ok_or_else(|| {
            ReadModelError::Storage(format!("read model version overflow for {key}"))
        }),
        None => Ok(INITIAL_MODEL_VERSION),
    }
}

pub(crate) fn apply_document_write_plan(
    plan: ReadModelWritePlan,
    staged_models: &mut HashMap<String, StoredModel>,
    staged_processed_messages: &mut ProcessedMessageSet,
) -> Result<ReadModelCommitOutcome, ReadModelError> {
    let capabilities = document_capabilities();
    plan.validate_for(&capabilities)?;

    let mut marks_in_plan = HashSet::with_capacity(plan.processed_messages.len());
    for mark in &plan.processed_messages {
        let key = processed_message_key(mark);
        if staged_processed_messages.contains(&key) || !marks_in_plan.insert(key) {
            return Ok(ReadModelCommitOutcome::skipped_duplicate(mark.clone()));
        }
    }

    for mutation in plan.mutations {
        if let ReadModelMutation::Document(mutation) = mutation {
            let key = mutation.key();
            let new_version = next_model_version(&key, staged_models.get(&key).map(|s| s.version))?;
            staged_models.insert(
                key,
                StoredModel {
                    bytes: mutation.bytes,
                    version: new_version,
                },
            );
        }
    }

    for mark in plan.processed_messages {
        staged_processed_messages.insert(processed_message_key(&mark));
    }

    Ok(ReadModelCommitOutcome::applied())
}

pub(crate) fn document_capabilities() -> ReadModelAdapterCapabilities {
    ReadModelAdapterCapabilities {
        relational_rows: false,
        document_rows: true,
        sparse_patches: false,
        deletes: false,
        processed_messages: true,
    }
}

fn processed_message_key(mark: &ProcessedMessageMark) -> (String, String) {
    (mark.consumer_name.clone(), mark.message_id.clone())
}

/// In-memory read model store backed by a HashMap.
///
/// Storage key is `"TABLE:id"`. Clone-friendly via Arc.
#[derive(Clone)]
pub struct InMemoryReadModelStore {
    pub(crate) storage: Arc<RwLock<HashMap<String, StoredModel>>>,
    pub(crate) processed_messages: Arc<RwLock<ProcessedMessageSet>>,
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
            processed_messages: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    fn make_key(table: &str, id: &str) -> String {
        format!("{}:{}", table, id)
    }

    /// Save pre-serialized document bytes by storage key for in-memory test setup.
    #[cfg(test)]
    pub(crate) fn save_document_bytes(
        &self,
        key: &str,
        bytes: Vec<u8>,
    ) -> Result<u64, ReadModelError> {
        let mut storage = self
            .storage
            .write()
            .map_err(|_| ReadModelError::Storage("lock poisoned".into()))?;

        let new_version = next_model_version(key, storage.get(key).map(|s| s.version))?;

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

impl ReadModelSessionStore for InMemoryReadModelStore {
    fn read_model_capabilities(&self) -> ReadModelAdapterCapabilities {
        document_capabilities()
    }

    fn commit_write_plan(
        &self,
        plan: ReadModelWritePlan,
    ) -> Result<ReadModelCommitOutcome, ReadModelError> {
        let mut storage = self
            .storage
            .write()
            .map_err(|_| ReadModelError::Storage("lock poisoned".into()))?;
        let mut processed_messages = self
            .processed_messages
            .write()
            .map_err(|_| ReadModelError::Storage("lock poisoned".into()))?;

        let mut staged_models = storage.clone();
        let mut staged_processed_messages = processed_messages.clone();
        let outcome =
            apply_document_write_plan(plan, &mut staged_models, &mut staged_processed_messages)?;

        if outcome.was_applied() {
            *storage = staged_models;
            *processed_messages = staged_processed_messages;
        }

        Ok(outcome)
    }

    fn is_processed(&self, consumer_name: &str, message_id: &str) -> Result<bool, ReadModelError> {
        let processed_messages = self
            .processed_messages
            .read()
            .map_err(|_| ReadModelError::Storage("lock poisoned".into()))?;
        Ok(processed_messages.contains(&(consumer_name.to_string(), message_id.to_string())))
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
        let bytes = serde_json::to_vec(model).map_err(|e| ReadModelError::Serde(e.to_string()))?;

        let mut storage = self
            .storage
            .write()
            .map_err(|_| ReadModelError::Storage("lock poisoned".into()))?;

        let new_version = next_model_version(&key, storage.get(&key).map(|s| s.version))?;

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
        let bytes = serde_json::to_vec(model).map_err(|e| ReadModelError::Serde(e.to_string()))?;

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
                version: INITIAL_MODEL_VERSION,
            },
        );

        Ok(Versioned {
            data: model.clone(),
            version: INITIAL_MODEL_VERSION,
        })
    }

    fn update<M: ReadModel>(
        &self,
        model: &M,
        expected_version: u64,
    ) -> Result<Versioned<M>, ReadModelError> {
        let key = Self::make_key(M::COLLECTION, model.id());
        let bytes = serde_json::to_vec(model).map_err(|e| ReadModelError::Serde(e.to_string()))?;

        let mut storage = self
            .storage
            .write()
            .map_err(|_| ReadModelError::Storage("lock poisoned".into()))?;

        let actual_version =
            storage
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

        let new_version = next_model_version(&key, Some(actual_version))?;
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
                let data = serde_json::from_slice::<M>(&stored.bytes)
                    .map_err(|e| ReadModelError::Serde(e.to_string()))?;
                if predicate(&data) {
                    results.push(Versioned {
                        data,
                        version: stored.version,
                    });
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
        let mut matched = None;

        for (key, stored) in storage.iter() {
            if key.starts_with(&prefix) {
                let data = serde_json::from_slice::<M>(&stored.bytes)
                    .map_err(|e| ReadModelError::Serde(e.to_string()))?;
                if matched.is_none() && predicate(&data) {
                    matched = Some(Versioned {
                        data,
                        version: stored.version,
                    });
                }
            }
        }

        Ok(matched)
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
    fn save_document_bytes_returns_error_on_version_overflow() {
        let store = InMemoryReadModelStore::new();
        let key = InMemoryReadModelStore::make_key(TestModel::COLLECTION, "1");
        let bytes = serde_json::to_vec(&TestModel {
            id: "1".into(),
            value: 1,
        })
        .unwrap();
        store.storage.write().unwrap().insert(
            key.clone(),
            StoredModel {
                bytes,
                version: u64::MAX,
            },
        );

        let err = store.save_document_bytes(&key, b"{}".to_vec()).unwrap_err();

        assert!(
            matches!(err, ReadModelError::Storage(message) if message.contains("version overflow"))
        );
    }

    #[test]
    fn upsert_returns_error_on_version_overflow() {
        let store = InMemoryReadModelStore::new();
        let model = TestModel {
            id: "1".into(),
            value: 1,
        };
        let key = InMemoryReadModelStore::make_key(TestModel::COLLECTION, model.id());
        let bytes = serde_json::to_vec(&model).unwrap();
        store.storage.write().unwrap().insert(
            key,
            StoredModel {
                bytes,
                version: u64::MAX,
            },
        );

        let err = store.upsert(&model).unwrap_err();

        assert!(
            matches!(err, ReadModelError::Storage(message) if message.contains("version overflow"))
        );
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
    fn update_returns_error_on_version_overflow() {
        let store = InMemoryReadModelStore::new();
        let model = TestModel {
            id: "1".into(),
            value: 1,
        };
        let key = InMemoryReadModelStore::make_key(TestModel::COLLECTION, model.id());
        let bytes = serde_json::to_vec(&model).unwrap();
        store.storage.write().unwrap().insert(
            key,
            StoredModel {
                bytes,
                version: u64::MAX,
            },
        );

        let err = store.update(&model, u64::MAX).unwrap_err();

        assert!(
            matches!(err, ReadModelError::Storage(message) if message.contains("version overflow"))
        );
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

        let results = store.find_models::<TestModel>(&|m| m.value > 8).unwrap();
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
    fn find_models_returns_error_for_corrupted_rows() {
        let store = InMemoryReadModelStore::new();
        store
            .save_document_bytes("test_models:bad", b"not valid json".to_vec())
            .unwrap();

        let err = store.find_models::<TestModel>(&|_| true).unwrap_err();

        assert!(matches!(err, ReadModelError::Serde(_)));
    }

    #[test]
    fn find_one_model_returns_error_for_corrupted_rows() {
        let store = InMemoryReadModelStore::new();
        store
            .save_document_bytes("test_models:bad", b"not valid json".to_vec())
            .unwrap();

        let err = store.find_one_model::<TestModel>(&|_| true).unwrap_err();

        assert!(matches!(err, ReadModelError::Serde(_)));
    }

    #[test]
    fn find_one_model_validates_rows_after_first_match() {
        let store = InMemoryReadModelStore::new();
        store
            .upsert(&TestModel {
                id: "1".into(),
                value: 20,
            })
            .unwrap();
        store
            .save_document_bytes("test_models:bad", b"not valid json".to_vec())
            .unwrap();

        let err = store
            .find_one_model::<TestModel>(&|m| m.value > 15)
            .unwrap_err();

        assert!(matches!(err, ReadModelError::Serde(_)));
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
