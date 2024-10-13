use std::time::SystemTime;
use crate::event_emitter::EventEmitter;

// CommandRecord holds information about a digested command
#[derive(Clone)]
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
    pub event_emitter: Option<EventEmitter>,
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
            event_emitter: Some(EventEmitter::new()),
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
        if let Some(emitter) = &self.event_emitter {
            for event in self.events_to_emit.drain(..) {
                emitter.emit(event.event_type(), event.get_data());
            }
        } else {
            eprintln!("Warning: No event emitter available to emit queued events");
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

    pub fn emit(&self, event: &str) {
        if let Some(emitter) = &self.event_emitter {
            emitter.emit(event, &()); // No data passed
        } else {
            eprintln!("Warning: No event emitter available to emit event: {}", event);
        }
    }

    pub fn on<F>(&self, event: String, listener: F)
    where
        F: Fn(&dyn std::any::Any) + Send + Sync + 'static,
    {
        if let Some(emitter) = &self.event_emitter {
            emitter.on(event, listener);
        } else {
            eprintln!("Warning: No event emitter available to register listener for event: {}", event);
        }
    }
}
