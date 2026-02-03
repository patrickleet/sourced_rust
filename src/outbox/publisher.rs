use std::fmt;
use std::sync::{Arc, Mutex};

use crate::EventEmitter;

use super::record::OutboxRecord;

pub trait OutboxPublisher {
    type Error: fmt::Display;
    fn publish(&mut self, record: &OutboxRecord) -> Result<(), Self::Error>;
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

pub struct LogPublisher {
    buffer: Option<Arc<Mutex<Vec<String>>>>,
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

    fn format_line(record: &OutboxRecord) -> String {
        format!(
            "[OUTBOX] {} v{} {} {}",
            record.aggregate_id, record.aggregate_version, record.event_type, record.payload
        )
    }
}

impl OutboxPublisher for LogPublisher {
    type Error = LogPublisherError;

    fn publish(&mut self, record: &OutboxRecord) -> Result<(), Self::Error> {
        let line = Self::format_line(record);
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

    fn publish(&mut self, record: &OutboxRecord) -> Result<(), Self::Error> {
        let _ = self
            .emitter
            .emit(&record.event_type, record.payload.clone());
        Ok(())
    }
}
