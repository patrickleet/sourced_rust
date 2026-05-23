use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use crate::entity::{Committable, Entity, EventRecord};
use crate::read_model::in_memory::apply_document_write_plan;
use crate::read_model::{
    InMemoryReadModelStore, ReadModel, ReadModelAdapterCapabilities, ReadModelCommitOutcome,
    ReadModelError, ReadModelSessionStore, ReadModelStore, ReadModelWritePlan, Versioned,
};
use crate::repository::{
    Commit, CommitBatch, GetMany, GetOne, RepositoryError, SnapshotWrite, TransactionalCommit,
};
use crate::snapshot::{InMemorySnapshotStore, SnapshotRecord, SnapshotStore};

/// In-memory repository implementation using HashMap.
///
/// This repository is cheap to clone because it uses `Arc<RwLock<...>>`
/// internally - cloning creates another handle to the same storage.
/// Also includes an embedded `InMemoryReadModelStore` for read model storage.
#[derive(Clone)]
pub struct HashMapRepository {
    event_store: Arc<RwLock<HashMap<String, Vec<EventRecord>>>>,
    model_store: InMemoryReadModelStore,
    snapshot_store: InMemorySnapshotStore,
}

impl Default for HashMapRepository {
    fn default() -> Self {
        Self::new()
    }
}

impl HashMapRepository {
    /// Create a new empty repository.
    pub fn new() -> Self {
        HashMapRepository {
            event_store: Arc::new(RwLock::new(HashMap::new())),
            model_store: InMemoryReadModelStore::new(),
            snapshot_store: InMemorySnapshotStore::new(),
        }
    }

    pub(crate) fn event_store(&self) -> &RwLock<HashMap<String, Vec<EventRecord>>> {
        self.event_store.as_ref()
    }

    /// Access the embedded read model store directly.
    pub fn model_store(&self) -> &InMemoryReadModelStore {
        &self.model_store
    }

    /// Access the embedded snapshot store directly.
    pub fn snapshot_store(&self) -> &InMemorySnapshotStore {
        &self.snapshot_store
    }
}

impl GetOne for HashMapRepository {
    fn get_one(&self, id: &str) -> Result<Option<Entity>, RepositoryError> {
        let storage = self
            .event_store
            .read()
            .map_err(|_| RepositoryError::LockPoisoned("read"))?;

        if let Some(events) = storage.get(id) {
            let mut entity = Entity::new();
            entity.set_id(id);
            entity.load_from_history(events.clone());
            Ok(Some(entity))
        } else {
            Ok(None)
        }
    }
}

impl GetMany for HashMapRepository {
    fn get_many(&self, ids: &[&str]) -> Result<Vec<Entity>, RepositoryError> {
        let mut entities = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(entity) = self.get_one(id)? {
                entities.push(entity);
            }
        }
        Ok(entities)
    }
}

impl Commit for HashMapRepository {
    fn commit<C: Committable + ?Sized>(&self, committable: &mut C) -> Result<(), RepositoryError> {
        let entities = committable.entities_mut();
        self.commit_batch(CommitBatch::new(entities))
    }
}

impl TransactionalCommit for HashMapRepository {
    fn commit_batch(&self, batch: CommitBatch<'_>) -> Result<(), RepositoryError> {
        reject_duplicate_streams(&batch.entities)?;

        let mut storage = self
            .event_store
            .write()
            .map_err(|_| RepositoryError::LockPoisoned("write"))?;
        let mut model_storage = self
            .model_store
            .storage
            .write()
            .map_err(|_| RepositoryError::LockPoisoned("read model write"))?;
        let mut processed_messages = self
            .model_store
            .processed_messages
            .write()
            .map_err(|_| RepositoryError::LockPoisoned("processed-message write"))?;
        let mut snapshot_storage = self
            .snapshot_store
            .storage
            .write()
            .map_err(|_| RepositoryError::LockPoisoned("snapshot write"))?;

        let mut staged_events = storage.clone();
        let mut staged_models = model_storage.clone();
        let mut staged_processed_messages = processed_messages.clone();
        let mut staged_snapshots = snapshot_storage.clone();

        // Phase 1: Validate all stream versions before staging any writes.
        for entity in &batch.entities {
            let stored_len = stored_stream_version(staged_events.get(entity.id()));
            if stored_len != entity.committed_version() {
                return Err(RepositoryError::ConcurrentWrite {
                    id: entity.id().to_string(),
                    expected: entity.committed_version(),
                    actual: stored_len,
                });
            }
        }

        // Phase 2: Apply every write to staged maps only.
        for entity in &batch.entities {
            let new_events = entity.new_events().to_vec();
            let stored = staged_events
                .entry(entity.id().to_string())
                .or_insert_with(Vec::new);
            stored.extend(new_events);
        }

        for plan in batch.read_model_plans {
            let outcome = apply_document_write_plan(
                plan,
                &mut staged_models,
                &mut staged_processed_messages,
            )?;
            if let Some(mark) = outcome.duplicate_message() {
                return Err(RepositoryError::Model(format!(
                    "processed message already handled by consumer `{}`: `{}`",
                    mark.consumer_name, mark.message_id
                )));
            }
        }

        for write in batch.snapshots {
            match write {
                SnapshotWrite::Save(record) => {
                    staged_snapshots.insert(record.aggregate_id.clone(), record);
                }
            }
        }

        // Phase 3: Publish staged state only after all validation and staging succeeds.
        *storage = staged_events;
        *model_storage = staged_models;
        *processed_messages = staged_processed_messages;
        *snapshot_storage = staged_snapshots;

        for entity in batch.entities {
            entity.mark_committed();
        }

        Ok(())
    }
}

fn reject_duplicate_streams(entities: &[&mut Entity]) -> Result<(), RepositoryError> {
    let mut seen = HashSet::with_capacity(entities.len());
    for entity in entities {
        let id = entity.id();
        if !seen.insert(id.to_string()) {
            return Err(RepositoryError::DuplicateStreamInBatch { id: id.to_string() });
        }
    }
    Ok(())
}

fn stored_stream_version(events: Option<&Vec<EventRecord>>) -> u64 {
    // A missing stream has committed version 0; the first appended event will
    // occupy sequence 1.
    events.map_or(0, |events| events.len() as u64)
}

impl ReadModelStore for HashMapRepository {
    fn get_model<M: ReadModel>(&self, id: &str) -> Result<Option<Versioned<M>>, ReadModelError> {
        self.model_store.get_model(id)
    }

    fn get_by_primary_key<M: ReadModel>(
        &self,
        id: &str,
    ) -> Result<Option<Versioned<M>>, ReadModelError> {
        self.model_store.get_by_primary_key(id)
    }

    fn upsert<M: ReadModel>(&self, model: &M) -> Result<Versioned<M>, ReadModelError> {
        self.model_store.upsert(model)
    }

    fn insert<M: ReadModel>(&self, model: &M) -> Result<Versioned<M>, ReadModelError> {
        self.model_store.insert(model)
    }

    fn update<M: ReadModel>(
        &self,
        model: &M,
        expected_version: u64,
    ) -> Result<Versioned<M>, ReadModelError> {
        self.model_store.update(model, expected_version)
    }

    fn delete<M: ReadModel>(&self, id: &str) -> Result<bool, ReadModelError> {
        self.model_store.delete::<M>(id)
    }

    fn find_models<M: ReadModel>(
        &self,
        predicate: &dyn Fn(&M) -> bool,
    ) -> Result<Vec<Versioned<M>>, ReadModelError> {
        self.model_store.find_models(predicate)
    }

    fn find_one_model<M: ReadModel>(
        &self,
        predicate: &dyn Fn(&M) -> bool,
    ) -> Result<Option<Versioned<M>>, ReadModelError> {
        self.model_store.find_one_model(predicate)
    }
}

impl ReadModelSessionStore for HashMapRepository {
    fn read_model_capabilities(&self) -> ReadModelAdapterCapabilities {
        self.model_store.read_model_capabilities()
    }

    fn commit_write_plan(
        &self,
        plan: ReadModelWritePlan,
    ) -> Result<ReadModelCommitOutcome, ReadModelError> {
        self.model_store.commit_write_plan(plan)
    }

    fn is_processed(&self, consumer_name: &str, message_id: &str) -> Result<bool, ReadModelError> {
        self.model_store.is_processed(consumer_name, message_id)
    }
}

impl SnapshotStore for HashMapRepository {
    fn get_snapshot(&self, id: &str) -> Result<Option<SnapshotRecord>, RepositoryError> {
        self.snapshot_store.get_snapshot(id)
    }

    fn save_snapshot(&self, record: SnapshotRecord) -> Result<(), RepositoryError> {
        self.snapshot_store.save_snapshot(record)
    }

    fn delete_snapshot(&self, id: &str) -> Result<bool, RepositoryError> {
        self.snapshot_store.delete_snapshot(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::read_model::in_memory::StoredModel;
    use crate::read_model::{DocumentMutation, ReadModelMutation};
    use crate::repository::Get;

    #[test]
    fn new() {
        let repo = HashMapRepository::new();
        assert!(repo.event_store.read().unwrap().is_empty());
    }

    #[test]
    fn single_entity_commit() {
        let repo = HashMapRepository::new();
        let id = "test_id";
        let mut entity = Entity::with_id(id);

        entity.digest("test_event", &("arg1", "arg2")).unwrap();

        repo.commit(&mut entity).unwrap();

        let fetched_entity = repo.get(id).unwrap().unwrap();
        assert_eq!(fetched_entity.id(), id);
        assert_eq!(fetched_entity.events(), entity.events());
    }

    #[test]
    fn multiple_entity_commit() {
        let repo = HashMapRepository::new();

        let mut entity1 = Entity::with_id("id_1");
        entity1.digest("event1", &"arg1").unwrap();

        let mut entity2 = Entity::with_id("id_2");
        entity2.digest("event2", &"arg2").unwrap();

        // Commit multiple entities using array syntax
        repo.commit(&mut [&mut entity1, &mut entity2]).unwrap();

        let all_entities: Vec<Entity> = repo.get(&["id_1", "id_2"]).unwrap();
        assert_eq!(all_entities.len(), 2);
    }

    #[test]
    fn duplicate_stream_ids_rejected_before_write() {
        let repo = HashMapRepository::new();

        let mut entity1 = Entity::with_id("same-id");
        entity1.digest("event1", &"arg1").unwrap();

        let mut entity2 = Entity::with_id("same-id");
        entity2.digest("event2", &"arg2").unwrap();

        let err = repo.commit(&mut [&mut entity1, &mut entity2]).unwrap_err();
        assert_eq!(
            err,
            RepositoryError::DuplicateStreamInBatch {
                id: "same-id".into()
            }
        );

        assert!(repo.get("same-id").unwrap().is_none());
        assert_eq!(entity1.committed_version(), 0);
        assert_eq!(entity2.committed_version(), 0);
        assert_eq!(entity1.new_events().len(), 1);
        assert_eq!(entity2.new_events().len(), 1);
    }

    #[test]
    fn document_plan_rejects_version_overflow_without_writing() {
        let repo = HashMapRepository::new();
        let key = "test_models:plan-overflow".to_string();
        let original_bytes = b"old".to_vec();
        repo.model_store.storage.write().unwrap().insert(
            key.clone(),
            StoredModel {
                bytes: original_bytes.clone(),
                version: u64::MAX,
            },
        );
        let plan = ReadModelWritePlan::new(
            vec![ReadModelMutation::Document(DocumentMutation {
                collection: "test_models".into(),
                id: "plan-overflow".into(),
                bytes: b"new".to_vec(),
            })],
            Vec::new(),
        );

        let err = repo
            .commit_batch(CommitBatch {
                entities: Vec::new(),
                read_model_plans: vec![plan],
                snapshots: Vec::new(),
            })
            .unwrap_err();

        assert!(
            matches!(err, RepositoryError::Model(message) if message.contains("version overflow"))
        );

        let storage = repo.model_store.storage.read().unwrap();
        let stored = storage.get(&key).unwrap();
        assert_eq!(stored.version, u64::MAX);
        assert_eq!(stored.bytes, original_bytes);
    }
}
