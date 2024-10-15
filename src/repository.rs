use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use crate::entity::{Entity, EventRecord};
use rayon::prelude::*; 

pub struct Repository {
    storage: Arc<RwLock<HashMap<String, Vec<EventRecord>>>>,
}

impl Repository {
    pub fn new() -> Self {
        Repository {
            storage: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn get(&self, id: &str) -> Option<Entity> {
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

    pub fn get_all(&self, ids: &[&str]) -> Vec<Entity> {
        ids.par_iter()
            .filter_map(|&id| self.get(id))
            .collect()
    }

    pub fn commit(&self, entity: &mut Entity) -> Result<(), String> {
        let mut storage = self.storage.write().unwrap();  // Write lock for modification
        
        // Store the event log
        storage.insert(entity.id.clone(), entity.events.clone());
        
        // Emit all queued events
        entity.emit_queued_events();
        
        Ok(())
    }

    pub fn commit_all(&self, entities: &mut [&mut Entity]) -> Result<(), String> {
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

impl Default for Repository {
    fn default() -> Self {
        Self::new()
    }
}
