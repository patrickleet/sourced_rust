use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::core::{
    Commit, Committable, Count, Entity, EventRecord, Exists, Find, FindOne, GetMany, GetOne,
    RepositoryError,
};

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

impl GetOne for HashMapRepository {
    fn get_one(&self, id: &str) -> Result<Option<Entity>, RepositoryError> {
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
            .storage
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
            .storage
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
            .storage
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
            .storage
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
    use crate::core::Get;

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

        entity.digest("test_event", &("arg1", "arg2"));

        repo.commit(&mut entity).unwrap();

        let fetched_entity = repo.get(id).unwrap().unwrap();
        assert_eq!(fetched_entity.id(), id);
        assert_eq!(fetched_entity.events(), entity.events());
    }

    #[test]
    fn multiple_entity_commit() {
        let repo = HashMapRepository::new();

        let mut entity1 = Entity::with_id("id_1");
        entity1.digest("event1", &"arg1");

        let mut entity2 = Entity::with_id("id_2");
        entity2.digest("event2", &"arg2");

        // Commit multiple entities using array syntax
        repo.commit(&mut [&mut entity1, &mut entity2]).unwrap();

        let all_entities: Vec<Entity> = repo.get(&["id_1", "id_2"]).unwrap();
        assert_eq!(all_entities.len(), 2);
    }

    #[test]
    fn find_returns_matching_entities() {
        let repo = HashMapRepository::new();

        let mut todo1 = Entity::with_id("todo-1");
        todo1.digest("Created", &"todo-1");

        let mut todo2 = Entity::with_id("todo-2");
        todo2.digest("Created", &"todo-2");

        let mut user1 = Entity::with_id("user-1");
        user1.digest("Created", &"user-1");

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
        entity1.digest("Created", &"item-1");

        let mut entity2 = Entity::with_id("item-2");
        entity2.digest("Created", &"item-2");

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
        entity.digest("Created", &"todo-1");
        repo.commit(&mut entity).unwrap();

        assert!(repo.exists(|e| e.id() == "todo-1").unwrap());
        assert!(!repo.exists(|e| e.id() == "todo-2").unwrap());
    }

    #[test]
    fn count_returns_matching_count() {
        let repo = HashMapRepository::new();

        let mut todo1 = Entity::with_id("todo-1");
        todo1.digest("Created", &"todo-1");

        let mut todo2 = Entity::with_id("todo-2");
        todo2.digest("Created", &"todo-2");

        let mut user1 = Entity::with_id("user-1");
        user1.digest("Created", &"user-1");

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
