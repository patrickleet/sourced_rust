use std::time::SystemTime;
use std::fmt;
use event_emitter_rs::EventEmitter;
use serde::{Serialize, Deserialize};
use crate::local_event::LocalEvent;
use crate::event_record::EventRecord;
use crate::event::Event;

#[derive(Serialize, Deserialize)]
pub struct Entity {
    pub id: String,
    pub version: i32,
    pub events: Vec<EventRecord>,
    #[serde(skip)]
    pub events_to_emit: Vec<LocalEvent>,
    pub replaying: bool,
    pub snapshot_version: i32,
    pub timestamp: SystemTime,
    #[serde(skip)]
    pub event_emitter: EventEmitter,
}

impl fmt::Debug for Entity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Entity")
            .field("id", &self.id)
            .field("version", &self.version)
            .field("events", &self.events)
            .field("events_to_emit", &self.events_to_emit)
            .field("replaying", &self.replaying)
            .field("snapshot_version", &self.snapshot_version)
            .field("timestamp", &self.timestamp)
            .finish()
    }
}

impl Clone for Entity {
    fn clone(&self) -> Self {
        Entity {
            id: self.id.clone(),
            version: self.version,
            events: self.events.clone(),
            events_to_emit: self.events_to_emit.clone(),
            replaying: self.replaying,
            snapshot_version: self.snapshot_version,
            timestamp: self.timestamp,
            event_emitter: EventEmitter::new(), // Create a new EventEmitter instead of cloning
        }
    }
}

impl Entity {
    pub fn new() -> Self {
        Entity {
            id: String::new(),
            version: 0,
            events: Vec::new(),
            events_to_emit: Vec::new(),
            replaying: false,
            snapshot_version: 0,
            timestamp: SystemTime::now(),
            event_emitter: EventEmitter::new(),
        }
    }

    pub fn digest(&mut self, name: String, args: Vec<String>) {
        if self.replaying {
            return;
        }
        self.events.push(EventRecord {
            event_name: name,
            args,
        });
        self.version += 1;
        self.timestamp = SystemTime::now();
    }

    pub fn enqueue(&mut self, event_type: String, data: String) {
        if self.replaying {
            return;
        }
        self.events_to_emit.push(LocalEvent {
            event_type,
            data,
        });
    }

    pub fn emit_queued_events(&mut self) {
        let events: Vec<_> = self.events_to_emit.drain(..).collect();
    
        for event in events {
            let event_type = event.event_type();  // Assuming this returns &str
            let data = event.get_data();          // Assuming this returns &str
            self.emit(event_type, data);
        }
    }

    pub fn rehydrate(&mut self) -> Result<(), String> {
        self.replaying = true;
    
        // Collect references to events first to avoid borrowing self immutably
        let events_to_replay: Vec<_> = self.events.iter().cloned().collect();
    
        for event in events_to_replay {
            self.replay_event(event)?;
        }
    
        self.replaying = false;
        Ok(())
    }

    fn replay_event(&mut self, event_record: EventRecord) -> Result<(), String> {
        println!("Replaying event: {} with args: {:?}", event_record.event_name, event_record.args);
        Ok(())
    }

    pub fn emit(&mut self, event: &str, data: &str) {
        self.event_emitter.emit(event, data.to_string());
    }

    pub fn on<F>(&mut self, event: &str, listener: F)
    where
        F: Fn(String) + Send + Sync + 'static,
    {
        self.event_emitter.on(event, listener);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    #[test]
    fn new() {
        // Test the default state of a new Entity instance
        // This test ensures that a new Entity is initialized with the correct default values
        let entity = Entity::new();
        assert_eq!(entity.id, String::new());
        assert_eq!(entity.version, 0);
        assert!(entity.events.is_empty());
        assert!(entity.events_to_emit.is_empty());
        assert!(!entity.replaying);
        assert_eq!(entity.snapshot_version, 0);
    }

    #[test]
    fn digest() {
        // Test the digest method to ensure it correctly updates the entity's state
        // This test checks that the digest method adds an event and increments the version
        let mut entity = Entity::new();
        entity.digest("test_event".to_string(), vec!["arg1".to_string(), "arg2".to_string()]);
        
        assert_eq!(entity.version, 1);
        assert_eq!(entity.events.len(), 1);
        assert_eq!(entity.events[0].event_name, "test_event");
        assert_eq!(entity.events[0].args, vec!["arg1", "arg2"]);
    }

    #[test]
    fn enqueue() {
        // Test the enqueue method to ensure it correctly queues events for emission
        // This test verifies that events are added to the events_to_emit vector
        let mut entity = Entity::new();
        entity.enqueue("test_event".to_string(), "test_data".to_string());
        
        assert_eq!(entity.events_to_emit.len(), 1);
        assert_eq!(entity.events_to_emit[0].event_type(), "test_event");
        assert_eq!(entity.events_to_emit[0].get_data(), "test_data");
    }

    #[test]
    fn emit_queued_events() {
        // Test the emit_queued_events method to ensure it processes all queued events
        // This test checks that events are emitted and the queue is cleared
        let mut entity = Entity::new();
        entity.enqueue("test_event1".to_string(), "test_data1".to_string());
        entity.enqueue("test_event2".to_string(), "test_data2".to_string());
        
        assert_eq!(entity.events_to_emit.len(), 2);
        
        entity.emit_queued_events();
        
        assert!(entity.events_to_emit.is_empty());
    }

    #[test]
    fn rehydrate() {
        // Test the rehydrate method to ensure it replays all events correctly
        // This test verifies that the rehydrate method processes all events without errors
        let mut entity = Entity::new();
        entity.digest("test_event1".to_string(), vec!["arg1".to_string()]);
        entity.digest("test_event2".to_string(), vec!["arg2".to_string()]);
        
        assert_eq!(entity.events.len(), 2);
        assert_eq!(entity.version, 2);
        
        let result = entity.rehydrate();
        
        assert!(result.is_ok());
        assert!(!entity.replaying);
    }

    #[test]
    fn clone() {
        // Test the Clone implementation for Entity
        // This test ensures that cloning an Entity creates a new instance with the same data
        let entity = Entity::new();
        let cloned_entity = entity.clone();
        
        assert_eq!(entity.id, cloned_entity.id);
        assert_eq!(entity.version, cloned_entity.version);
        assert_eq!(entity.events, cloned_entity.events);
        assert_eq!(entity.events_to_emit, cloned_entity.events_to_emit);
        assert_eq!(entity.replaying, cloned_entity.replaying);
        assert_eq!(entity.snapshot_version, cloned_entity.snapshot_version);
    }

    #[test]
    fn debug() {
        // Test the Debug implementation for Entity
        // This test checks that the debug string representation of an Entity is formatted correctly
        let entity = Entity::new();
        let debug_str = format!("{:?}", entity);
        
        assert!(debug_str.contains("Entity"));
        assert!(debug_str.contains("id: \"\""));
        assert!(debug_str.contains("version: 0"));
    }

    #[test]
    fn serialize_deserialize() {
        // Test the Serialize and Deserialize implementations for Entity
        // This test ensures that an Entity can be serialized to JSON and deserialized back without data loss
        let entity = Entity::new();
        let serialized: String = serde_json::to_string(&entity).unwrap();
        let deserialized: Entity = serde_json::from_str(&serialized).unwrap();
        
        assert_eq!(entity.id, deserialized.id);
        assert_eq!(entity.version, deserialized.version);
        assert_eq!(entity.events, deserialized.events);
        assert_eq!(entity.replaying, deserialized.replaying);
        assert_eq!(entity.snapshot_version, deserialized.snapshot_version);
    }

    #[test]
    fn emit_and_on() {
        // Test the emit method to ensure it emits events correctly
        let mut entity = Entity::new();
        let event_name = "test_event";
        let event_data = "test_data";

        entity.on(event_name, move |data| {
            assert_eq!(*event_data, data);
        });

        entity.emit(event_name, event_data);
    }

    #[test]
    fn replaying_state() {
        // Test edge cases when the entity is in replaying state
        let mut entity = Entity::new();
        entity.replaying = true;

        // Ensure digest does not modify state when replaying
        entity.digest("test_event".to_string(), vec!["arg1".to_string()]);
        assert!(entity.events.is_empty());

        // Ensure enqueue does not modify state when replaying
        entity.enqueue("test_event".to_string(), "test_data".to_string());
        assert!(entity.events_to_emit.is_empty());
    }
}
