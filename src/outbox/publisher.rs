use std::fmt;
use std::sync::{Arc, Mutex};

use crate::EventEmitter;

/// Trait for publishing outbox records to external systems.
pub trait OutboxPublisher {
    type Error: fmt::Display;

    /// Publish an event with the given type and payload bytes.
    /// The publisher is responsible for converting the payload to the appropriate format
    /// (e.g., decoding bitcode and re-encoding to JSON for CloudEvents).
    fn publish(&mut self, event_type: &str, payload: &[u8]) -> Result<(), Self::Error>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogPublisherError {
    BufferPoisoned,
}

impl fmt::Display for LogPublisherError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LogPublisherError::BufferPoisoned => write!(f, "log publisher buffer poisoned"),
        }
    }
}

impl std::error::Error for LogPublisherError {}

/// A simple publisher that logs events to stdout or a buffer.
pub struct LogPublisher {
    buffer: Option<Arc<Mutex<Vec<String>>>>,
}

impl Default for LogPublisher {
    fn default() -> Self {
        Self::new()
    }
}

impl LogPublisher {
    pub fn new() -> Self {
        LogPublisher { buffer: None }
    }

    pub fn with_buffer(buffer: Arc<Mutex<Vec<String>>>) -> Self {
        LogPublisher {
            buffer: Some(buffer),
        }
    }
}

impl OutboxPublisher for LogPublisher {
    type Error = LogPublisherError;

    fn publish(&mut self, event_type: &str, payload: &[u8]) -> Result<(), Self::Error> {
        let payload_str = String::from_utf8_lossy(payload);
        let line = format!("[OUTBOX] {} {}", event_type, payload_str);
        if let Some(buffer) = &self.buffer {
            let mut buffer = buffer
                .lock()
                .map_err(|_| LogPublisherError::BufferPoisoned)?;
            buffer.push(line);
        } else {
            println!("{}", line);
        }
        Ok(())
    }
}

/// A publisher that emits events via an EventEmitter for in-process subscribers.
pub struct LocalEmitterPublisher {
    emitter: EventEmitter,
}

impl LocalEmitterPublisher {
    pub fn new(emitter: EventEmitter) -> Self {
        LocalEmitterPublisher { emitter }
    }
}

impl OutboxPublisher for LocalEmitterPublisher {
    type Error = std::convert::Infallible;

    fn publish(&mut self, event_type: &str, payload: &[u8]) -> Result<(), Self::Error> {
        // Convert bytes to string for the event emitter (assumes UTF-8)
        let payload_str = String::from_utf8_lossy(payload).into_owned();
        self.emitter.emit(event_type, payload_str);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_publisher_to_buffer() {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let mut publisher = LogPublisher::with_buffer(buffer.clone());

        publisher.publish("UserCreated", br#"{"id":"123"}"#).unwrap();
        publisher.publish("UserUpdated", br#"{"id":"123","name":"Alice"}"#).unwrap();

        let logs = buffer.lock().unwrap();
        assert_eq!(logs.len(), 2);
        assert!(logs[0].contains("UserCreated"));
        assert!(logs[1].contains("UserUpdated"));
    }
}
