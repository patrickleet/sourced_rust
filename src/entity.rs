use std::time::SystemTime;
use event_emitter_rs::EventEmitter;
use serde::{Serialize, Deserialize};

// CommandRecord holds information about a digested command
#[derive(Clone, Serialize, Deserialize)]
pub struct CommandRecord {
    pub command_name: String,
    pub args: Vec<String>,
}

// Event trait defines an event type for the system
pub trait Event: Send + Sync {
    fn event_type(&self) -> &str;
    fn get_data(&self) -> &dyn std::any::Any;
}

// LocalEvent struct implements the Event trait
pub struct LocalEvent {
    event_type: String,
    data: Box<dyn std::any::Any + Send + Sync>,
}

impl Event for LocalEvent {
    fn event_type(&self) -> &str {
        &self.event_type
    }

    fn get_data(&self) -> &dyn std::any::Any {
        self.data.as_ref()
    }
}

// Entity struct defines the base entity that all domain models will extend
pub struct Entity {
    pub id: String,
    pub version: i32,
    pub commands: Vec<CommandRecord>,
    pub events_to_emit: Vec<Box<dyn Event>>,
    pub replaying: bool,
    pub snapshot_version: i32,
    pub timestamp: SystemTime,
    pub event_emitter: EventEmitter,
}

impl Entity {
    pub fn new() -> Self {
        Entity {
            id: String::new(),
            version: 0,
            commands: Vec::new(),
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
        self.commands.push(CommandRecord {
            command_name: name,
            args,
        });
        self.version += 1;
        self.timestamp = SystemTime::now();
    }

    pub fn enqueue(&mut self, event_type: String) {
        if self.replaying {
            return;
        }
        self.events_to_emit.push(Box::new(LocalEvent {
            event_type,
            data: Box::new(()), // No data passed
        }));
    }

    pub fn emit_queued_events(&mut self) {
        let event_types: Vec<String> = self.events_to_emit.drain(..).map(|event| event.event_type().to_string()).collect();
        for event_type in event_types {
            self.emit(&event_type);
        }
    }

    pub fn rehydrate(&mut self) -> Result<(), String> {
        self.replaying = true;
        for command in self.commands.clone() {
            if let Err(e) = self.replay_command(command) {
                self.replaying = false;
                return Err(format!("Error replaying command: {}", e));
            }
        }
        self.replaying = false;
        Ok(())
    }

    fn replay_command(&mut self, command_record: CommandRecord) -> Result<(), String> {
        // This method should be overridden by domain-specific models
        // For now, we'll just log the command being replayed
        println!("Replaying command: {} with args: {:?}", command_record.command_name, command_record.args);
        Ok(())
    }

    pub fn emit(&mut self, event: &str) {
        self.event_emitter.emit(event, ());
    }

    pub fn on<F>(&mut self, event: &str, listener: F)
    where
        F: Fn(()) + Send + Sync + 'static,
    {
        self.event_emitter.on(event, listener);
    }
}
