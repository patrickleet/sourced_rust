use std::time::SystemTime;
use std::fmt;
use event_emitter_rs::EventEmitter;
use serde::{Serialize, Deserialize};

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct EventRecord {
    pub event_name: String,
    pub args: Vec<String>,
}

pub trait Event: Send + Sync {
    fn event_type(&self) -> &str;
    fn get_data(&self) -> &str;
}

#[derive(Clone, Debug)]
pub struct LocalEvent {
    event_type: String,
    data: String,
}

impl Event for LocalEvent {
    fn event_type(&self) -> &str {
        &self.event_type
    }

    fn get_data(&self) -> &str {
        &self.data
    }
}

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
        let events: Vec<(String, String)> = self.events_to_emit.drain(..)
            .map(|event| (event.event_type().to_string(), event.get_data().to_string()))
            .collect();
        for (event_type, data) in events {
            self.emit(&event_type, &data);
        }
    }

    pub fn rehydrate(&mut self) -> Result<(), String> {
        self.replaying = true;
    
        // Collect references to events first to avoid borrowing self immutably
        let events_to_replay: Vec<_> = self.events.iter().cloned().collect();
    
        for event in events_to_replay {
            if let Err(e) = self.replay_event(event) {
                self.replaying = false;
                return Err(format!("Error replaying event: {}", e));
            }
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
