use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

#[cfg(feature = "emitter")]
use crate::EventEmitter;

/// Trait for publishing outbox records to external systems.
pub trait OutboxPublisher {
    type Error: fmt::Display;

    /// Publish an event with the given type, payload bytes, and metadata.
    /// The publisher is responsible for converting the payload to the appropriate format
    /// (e.g., decoding bitcode and re-encoding to JSON for CloudEvents).
    fn publish(
        &mut self,
        event_type: &str,
        payload: &[u8],
        metadata: &HashMap<String, String>,
    ) -> Result<(), Self::Error>;
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

    fn publish(
        &mut self,
        event_type: &str,
        payload: &[u8],
        metadata: &HashMap<String, String>,
    ) -> Result<(), Self::Error> {
        let payload_str = String::from_utf8_lossy(payload);
        let meta_str = if metadata.is_empty() {
            String::new()
        } else {
            format!(" meta={:?}", metadata)
        };
        let line = format!("[OUTBOX] {} {}{}", event_type, payload_str, meta_str);
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
/// Requires the `emitter` feature to be enabled.
#[cfg(feature = "emitter")]
pub struct LocalEmitterPublisher {
    emitter: EventEmitter,
}

#[cfg(feature = "emitter")]
impl LocalEmitterPublisher {
    pub fn new(emitter: EventEmitter) -> Self {
        LocalEmitterPublisher { emitter }
    }
}

#[cfg(feature = "emitter")]
impl OutboxPublisher for LocalEmitterPublisher {
    type Error = std::convert::Infallible;

    fn publish(
        &mut self,
        event_type: &str,
        payload: &[u8],
        _metadata: &HashMap<String, String>,
    ) -> Result<(), Self::Error> {
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
        let empty = HashMap::new();

        publisher.publish("UserCreated", br#"{"id":"123"}"#, &empty).unwrap();
        publisher.publish("UserUpdated", br#"{"id":"123","name":"Alice"}"#, &empty).unwrap();

        let logs = buffer.lock().unwrap();
        assert_eq!(logs.len(), 2);
        assert!(logs[0].contains("UserCreated"));
        assert!(logs[1].contains("UserUpdated"));
    }

    #[test]
    fn log_publisher_includes_metadata() {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let mut publisher = LogPublisher::with_buffer(buffer.clone());
        let mut meta = HashMap::new();
        meta.insert("correlation_id".to_string(), "req-123".to_string());

        publisher.publish("UserCreated", br#"{}"#, &meta).unwrap();

        let logs = buffer.lock().unwrap();
        assert!(logs[0].contains("correlation_id"));
        assert!(logs[0].contains("req-123"));
    }
}
