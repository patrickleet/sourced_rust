use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::core::{Committable, Entity, EventRecord, Repository, RepositoryError};

/// In-memory repository implementation using HashMap.

pub struct HashMapRepository {
    storage: Arc<RwLock<HashMap<String, Vec<EventRecord>>>>,
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
            storage: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub(crate) fn storage(&self) -> &RwLock<HashMap<String, Vec<EventRecord>>> {
        self.storage.as_ref()
    }
}

impl Repository for HashMapRepository {
    fn get(&self, id: &str) -> Result<Option<Entity>, RepositoryError> {
        let storage = self
            .storage
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

    fn get_all(&self, ids: &[&str]) -> Result<Vec<Entity>, RepositoryError> {
        let mut entities = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(entity) = self.get(id)? {
                entities.push(entity);
            }
        }
        Ok(entities)
    }

    fn commit<C: Committable + ?Sized>(&self, committable: &mut C) -> Result<(), RepositoryError> {
        let entities = committable.entities_mut();

        let mut storage = self
            .storage
            .write()
            .map_err(|_| RepositoryError::LockPoisoned("write"))?;

        for entity in &entities {
            storage.insert(entity.id().to_string(), entity.events().to_vec());
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new() {
        let repo = HashMapRepository::new();
        assert!(repo.storage.read().unwrap().is_empty());
    }

    #[test]
    fn single_entity_commit() {
        let repo = HashMapRepository::new();
        let id = "test_id";
        let mut entity = Entity::with_id(id);

        let args = vec!["arg1".to_string(), "arg2".to_string()];
        entity.digest("test_event", args);

        repo.commit(&mut entity).unwrap();

        let fetched_entity = repo.get(id).unwrap().unwrap();
        assert_eq!(fetched_entity.id(), id);
        assert_eq!(fetched_entity.events(), entity.events());
    }

    #[test]
    fn multiple_entity_commit() {
        let repo = HashMapRepository::new();

        let mut entity1 = Entity::with_id("id_1");
        entity1.digest("event1", vec!["arg1".to_string()]);

        let mut entity2 = Entity::with_id("id_2");
        entity2.digest("event2", vec!["arg2".to_string()]);

        // Commit multiple entities using array syntax
        repo.commit(&mut [&mut entity1, &mut entity2]).unwrap();

        let all_entities = repo.get_all(&["id_1", "id_2"]).unwrap();
        assert_eq!(all_entities.len(), 2);
    }

}
