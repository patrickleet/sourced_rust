use std::sync::{Arc, RwLock};
use std::collections::HashMap;
use crate::event_record::EventRecord;
use crate::entity::Entity;
use crate::Repository;
use rayon::prelude::*; 


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

    fn get(&self, id: &str) -> Option<Entity> {
        let storage = self.storage.read().unwrap();  // Read lock
        
        if let Some(events) = storage.get(id) {
            let mut entity = Entity::new();
            entity.id = id.to_string();
            entity.events = events.clone();
            
            // Rehydrate the entity
            if let Err(e) = entity.rehydrate() {
                eprintln!("Error rehydrating entity: {}", e);
                return None;
            }
            
            Some(entity)
        } else {
            None
        }
    }

    fn get_all(&self, ids: &[&str]) -> Vec<Entity> {
        ids.par_iter()
            .filter_map(|&id| self.get(id))
            .collect()
    }

    fn commit(&self, entity: &mut Entity) -> Result<(), String> {
        let mut storage = self.storage.write().unwrap();  // Write lock for modification
        
        // Store the event log
        storage.insert(entity.id.clone(), entity.events.clone());
        
        // Emit all queued events
        entity.emit_queued_events();
        
        Ok(())
    }

    fn commit_all(&self, entities: &mut [&mut Entity]) -> Result<(), String> {
        let new_items: HashMap<String, Vec<EventRecord>> = entities
            .par_iter()
            .map(|entity| (entity.id.clone(), entity.events.clone()))  // Create key-value pairs in parallel
            .collect();  // Collect into a HashMap
    
        let mut storage = self.storage.write().unwrap();  // Write lock for modification
        storage.extend(new_items);  // Merge the new items into storage
    
        // Emit events in parallel
        entities.par_iter_mut().for_each(|entity| {
            entity.emit_queued_events();
        });
    
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::Entity;

    #[test]
    fn test_new() {
        let repo = HashMapRepository::new();
        assert!(repo.storage.read().unwrap().is_empty());
    }

    #[test]
    fn test_workflow() {
        let repo = HashMapRepository::new();
        let id = "test_id";
        let mut entity = Entity::new();
        entity.id = id.to_string();

        let args = vec!["arg1".to_string(), "arg2".to_string()];
        entity.digest("test_event".to_string(), args);

        entity.enqueue("test_event".to_string(), "test_data".to_string());

        entity.on("test_event", |data| {
            println!("Event listener called with data: {}", data);
            assert!(data == "test_data");
        });

        repo.commit(&mut entity).unwrap();

        let fetched_entity = repo.get(id).unwrap();
        assert_eq!(fetched_entity.id, id);
        assert_eq!(fetched_entity.events, entity.events);

        let args2 = vec!["arg1".to_string(), "arg2".to_string()];

        let mut entity2 = Entity::new();
        entity2.id = "test_id_2".to_string();
        entity2.digest("test_event".to_string(), args2);

        let result = repo.commit_all(&mut [&mut entity, &mut entity2]);
        assert!(result.is_ok());

        let all_entities = repo.get_all(&[id, "test_id_2"]);
        assert_eq!(all_entities.len(), 2);
    }
}

