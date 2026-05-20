use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use crate::entity::{Committable, Entity, EventRecord};
use crate::read_model::{
    InMemoryReadModelStore, ReadModel, ReadModelError, ReadModelStore, Versioned,
};
use crate::repository::{
    Commit, CommitBatch, Count, Exists, Find, FindOne, GetMany, GetOne, RepositoryError,
    SnapshotWrite, TransactionalCommit,
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

impl Find for HashMapRepository {
    fn find<F>(&self, predicate: F) -> Result<Vec<Entity>, RepositoryError>
    where
        F: Fn(&Entity) -> bool,
    {
        let storage = self
            .event_store
            .read()
            .map_err(|_| RepositoryError::LockPoisoned("read"))?;

        let mut results = Vec::new();
        for (id, events) in storage.iter() {
            let mut entity = Entity::new();
            entity.set_id(id);
            entity.load_from_history(events.clone());
            if predicate(&entity) {
                results.push(entity);
            }
        }
        Ok(results)
    }
}

impl FindOne for HashMapRepository {
    fn find_one<F>(&self, predicate: F) -> Result<Option<Entity>, RepositoryError>
    where
        F: Fn(&Entity) -> bool,
    {
        let storage = self
            .event_store
            .read()
            .map_err(|_| RepositoryError::LockPoisoned("read"))?;

        for (id, events) in storage.iter() {
            let mut entity = Entity::new();
            entity.set_id(id);
            entity.load_from_history(events.clone());
            if predicate(&entity) {
                return Ok(Some(entity));
            }
        }
        Ok(None)
    }
}

impl Exists for HashMapRepository {
    fn exists<F>(&self, predicate: F) -> Result<bool, RepositoryError>
    where
        F: Fn(&Entity) -> bool,
    {
        let storage = self
            .event_store
            .read()
            .map_err(|_| RepositoryError::LockPoisoned("read"))?;

        for (id, events) in storage.iter() {
            let mut entity = Entity::new();
            entity.set_id(id);
            entity.load_from_history(events.clone());
            if predicate(&entity) {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

impl Count for HashMapRepository {
    fn count<F>(&self, predicate: F) -> Result<usize, RepositoryError>
    where
        F: Fn(&Entity) -> bool,
    {
        let storage = self
            .event_store
            .read()
            .map_err(|_| RepositoryError::LockPoisoned("read"))?;

        let mut count = 0;
        for (id, events) in storage.iter() {
            let mut entity = Entity::new();
            entity.set_id(id);
            entity.load_from_history(events.clone());
            if predicate(&entity) {
                count += 1;
            }
        }
        Ok(count)
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
        let mut snapshot_storage = self
            .snapshot_store
            .storage
            .write()
            .map_err(|_| RepositoryError::LockPoisoned("snapshot write"))?;

        let mut staged_events = storage.clone();
        let mut staged_models = model_storage.clone();
        let mut staged_snapshots = snapshot_storage.clone();

        // Phase 1: Validate all stream versions before staging any writes.
        for entity in &batch.entities {
            let stored_len = staged_events
                .get(entity.id())
                .map(|v| v.len() as u64)
                .unwrap_or(0);
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

        for write in batch.read_models {
            let new_version = staged_models
                .get(&write.key)
                .map(|s| s.version + 1)
                .unwrap_or(1);
            staged_models.insert(
                write.key,
                crate::read_model::in_memory::StoredModel {
                    bytes: write.bytes,
                    version: new_version,
                },
            );
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

impl ReadModelStore for HashMapRepository {
    fn get_model<M: ReadModel>(&self, id: &str) -> Result<Option<Versioned<M>>, ReadModelError> {
        self.model_store.get_model(id)
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

    fn upsert_raw(&self, key: &str, bytes: Vec<u8>) -> Result<(), ReadModelError> {
        self.model_store.upsert_raw(key, bytes)
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
    fn find_returns_matching_entities() {
        let repo = HashMapRepository::new();

        let mut todo1 = Entity::with_id("todo-1");
        todo1.digest("Created", &"todo-1").unwrap();

        let mut todo2 = Entity::with_id("todo-2");
        todo2.digest("Created", &"todo-2").unwrap();

        let mut user1 = Entity::with_id("user-1");
        user1.digest("Created", &"user-1").unwrap();

        repo.commit(&mut [&mut todo1, &mut todo2, &mut user1])
            .unwrap();

        let todos = repo.find(|e| e.id().starts_with("todo-")).unwrap();
        assert_eq!(todos.len(), 2);

        let users = repo.find(|e| e.id().starts_with("user-")).unwrap();
        assert_eq!(users.len(), 1);

        let none = repo.find(|e| e.id().starts_with("order-")).unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn find_one_returns_first_match() {
        let repo = HashMapRepository::new();

        let mut entity1 = Entity::with_id("item-1");
        entity1.digest("Created", &"item-1").unwrap();

        let mut entity2 = Entity::with_id("item-2");
        entity2.digest("Created", &"item-2").unwrap();

        repo.commit(&mut [&mut entity1, &mut entity2]).unwrap();

        let found = repo.find_one(|e| e.id().starts_with("item-")).unwrap();
        assert!(found.is_some());
        assert!(found.unwrap().id().starts_with("item-"));

        let not_found = repo.find_one(|e| e.id().starts_with("missing-")).unwrap();
        assert!(not_found.is_none());
    }

    #[test]
    fn find_on_empty_repo() {
        let repo = HashMapRepository::new();

        let results = repo.find(|_| true).unwrap();
        assert!(results.is_empty());

        let result = repo.find_one(|_| true).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn exists_returns_true_when_match_found() {
        let repo = HashMapRepository::new();

        let mut entity = Entity::with_id("todo-1");
        entity.digest("Created", &"todo-1").unwrap();
        repo.commit(&mut entity).unwrap();

        assert!(repo.exists(|e| e.id() == "todo-1").unwrap());
        assert!(!repo.exists(|e| e.id() == "todo-2").unwrap());
    }

    #[test]
    fn count_returns_matching_count() {
        let repo = HashMapRepository::new();

        let mut todo1 = Entity::with_id("todo-1");
        todo1.digest("Created", &"todo-1").unwrap();

        let mut todo2 = Entity::with_id("todo-2");
        todo2.digest("Created", &"todo-2").unwrap();

        let mut user1 = Entity::with_id("user-1");
        user1.digest("Created", &"user-1").unwrap();

        repo.commit(&mut [&mut todo1, &mut todo2, &mut user1])
            .unwrap();

        assert_eq!(repo.count(|e| e.id().starts_with("todo-")).unwrap(), 2);
        assert_eq!(repo.count(|e| e.id().starts_with("user-")).unwrap(), 1);
        assert_eq!(repo.count(|_| true).unwrap(), 3);
        assert_eq!(repo.count(|e| e.id().starts_with("order-")).unwrap(), 0);
    }

    #[test]
    fn exists_and_count_on_empty_repo() {
        let repo = HashMapRepository::new();

        assert!(!repo.exists(|_| true).unwrap());
        assert_eq!(repo.count(|_| true).unwrap(), 0);
    }
}
