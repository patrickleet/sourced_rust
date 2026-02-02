use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::entity::Entity;
use crate::event_record::EventRecord;
use crate::error::RepositoryError;
use crate::Repository;

pub struct HashMapRepository {
    storage: Arc<RwLock<HashMap<String, Vec<EventRecord>>>>,
}

impl HashMapRepository {
    pub fn new() -> Self {
        HashMapRepository {
            storage: Arc::new(RwLock::new(HashMap::new())),
        }
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

    fn commit(&self, entity: &mut Entity) -> Result<(), RepositoryError> {
        let mut storage = self
            .storage
            .write()
            .map_err(|_| RepositoryError::LockPoisoned("write"))?;
        storage.insert(entity.id().to_string(), entity.events().to_vec());
        entity.emit_queued_events();

        Ok(())
    }

    fn commit_all(&self, entities: &mut [&mut Entity]) -> Result<(), RepositoryError> {
        let mut storage = self
            .storage
            .write()
            .map_err(|_| RepositoryError::LockPoisoned("write"))?;

        for entity in entities.iter_mut() {
            storage.insert(entity.id().to_string(), entity.events().to_vec());
            entity.emit_queued_events();
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
    fn full_workflow() {
        let repo = HashMapRepository::new();
        let id = "test_id";
        let mut entity = Entity::with_id(id);

        let args = vec!["arg1".to_string(), "arg2".to_string()];
        entity.digest("test_event", args);

        entity.enqueue("test_event", "test_data");

        entity.on("test_event", |data| {
            assert!(data == "test_data");
        });

        repo.commit(&mut entity).unwrap();

        let fetched_entity = repo.get(id).unwrap().unwrap();
        assert_eq!(fetched_entity.id(), id);
        assert_eq!(fetched_entity.events(), entity.events());

        let args2 = vec!["arg1".to_string(), "arg2".to_string()];

        let mut entity2 = Entity::with_id("test_id_2");
        entity2.digest("test_event", args2);

        let result = repo.commit_all(&mut [&mut entity, &mut entity2]);
        assert!(result.is_ok());

        let all_entities = repo.get_all(&[id, "test_id_2"]).unwrap();
        assert_eq!(all_entities.len(), 2);
    }
}
