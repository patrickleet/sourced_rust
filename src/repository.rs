use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use crate::entity::{Entity, CommandRecord};
use crate::event_emitter::EventEmitter;

pub struct Repository {
    storage: Arc<Mutex<HashMap<String, Vec<CommandRecord>>>>,
}

impl Repository {
    pub fn new() -> Self {
        Repository {
            storage: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn find_by_id(&self, id: &str) -> Option<Entity> {
        let storage = self.storage.lock().unwrap();
        
        if let Some(commands) = storage.get(id) {
            let mut entity = Entity::new();
            entity.id = id.to_string();
            entity.commands = commands.clone();
            
            // Initialize EventEmitter
            entity.event_emitter = Some(EventEmitter::new());
            
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

    pub fn commit(&self, entity: &mut Entity) -> Result<(), String> {
        let mut storage = self.storage.lock().unwrap();
        
        // Store the command log
        storage.insert(entity.id.clone(), entity.commands.clone());
        
        // Emit all queued events
        entity.emit_queued_events();
        
        Ok(())
    }
}

impl Default for Repository {
    fn default() -> Self {
        Self::new()
    }
}
