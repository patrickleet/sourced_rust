//! Core publisher traits for the service bus.

use std::error::Error;
use std::fmt;

/// An event to be published to the bus.
#[derive(Clone, Debug)]
pub struct Event {
    /// Unique identifier for this event
    pub id: String,
    /// Event type (e.g., "OrderCreated", "PaymentSucceeded")
    pub event_type: String,
    /// Serialized payload (typically JSON or binary)
    pub payload: Vec<u8>,
    /// Optional metadata (headers, correlation IDs, etc.)
    pub metadata: Option<Vec<(String, String)>>,
}

impl Event {
    /// Create a new event with the given type and payload.
    pub fn new(id: impl Into<String>, event_type: impl Into<String>, payload: Vec<u8>) -> Self {
        Self {
            id: id.into(),
            event_type: event_type.into(),
            payload,
            metadata: None,
        }
    }

    /// Create an event with bitcode-serialized payload.
    pub fn encode<T: serde::Serialize>(
        id: impl Into<String>,
        event_type: impl Into<String>,
        payload: &T,
    ) -> Result<Self, bitcode::Error> {
        let bytes = bitcode::serialize(payload)?;
        Ok(Self::new(id, event_type, bytes))
    }

    /// Decode the payload from bitcode binary format.
    pub fn decode<T: serde::de::DeserializeOwned>(&self) -> Result<T, bitcode::Error> {
        bitcode::deserialize(&self.payload)
    }

    /// Create an event with a string payload.
    pub fn with_string_payload(
        id: impl Into<String>,
        event_type: impl Into<String>,
        payload: impl Into<String>,
    ) -> Self {
        Self::new(id, event_type, payload.into().into_bytes())
    }

    /// Add metadata to the event.
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata
            .get_or_insert_with(Vec::new)
            .push((key.into(), value.into()));
        self
    }

    /// Get the payload as a string (if valid UTF-8).
    pub fn payload_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.payload).ok()
    }
}

/// Error type for publish operations.
#[derive(Debug)]
pub enum PublishError {
    /// Connection to the bus failed
    ConnectionFailed(String),
    /// Serialization of the event failed
    SerializationFailed(String),
    /// The bus rejected the event
    Rejected(String),
    /// Timeout waiting for acknowledgment
    Timeout,
    /// Other error
    Other(Box<dyn Error + Send + Sync>),
}

impl fmt::Display for PublishError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PublishError::ConnectionFailed(msg) => write!(f, "Connection failed: {}", msg),
            PublishError::SerializationFailed(msg) => write!(f, "Serialization failed: {}", msg),
            PublishError::Rejected(msg) => write!(f, "Event rejected: {}", msg),
            PublishError::Timeout => write!(f, "Publish timeout"),
            PublishError::Other(e) => write!(f, "Publish error: {}", e),
        }
    }
}

impl Error for PublishError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            PublishError::Other(e) => Some(e.as_ref()),
            _ => None,
        }
    }
}

/// Trait for publishing events to a message bus.
///
/// Implementations might include:
/// - `InMemoryBus` - For testing and single-process scenarios
/// - `KafkaPublisher` - For Apache Kafka
/// - `NatsPublisher` - For NATS
/// - `RabbitMqPublisher` - For RabbitMQ
/// - `CloudEventsPublisher` - For CloudEvents-compatible systems
pub trait Publisher: Send + Sync {
    /// Publish a single event to the bus.
    fn publish(&self, event: Event) -> Result<(), PublishError>;

    /// Publish multiple events to the bus.
    ///
    /// Default implementation publishes events sequentially.
    /// Implementations may override for batch optimization.
    fn publish_batch(&self, events: Vec<Event>) -> Result<(), PublishError> {
        for event in events {
            self.publish(event)?;
        }
        Ok(())
    }
}

/// Trait for subscribing to events from a message bus.
///
/// This is a pull-based interface. Implementations may also provide
/// push-based interfaces via callbacks or async streams.
pub trait Subscriber: Send + Sync {
    /// Poll for the next event, blocking until one is available or timeout.
    fn poll(&self, timeout_ms: u64) -> Result<Option<Event>, PublishError>;

    /// Acknowledge that an event has been processed.
    fn ack(&self, event_id: &str) -> Result<(), PublishError>;

    /// Reject an event (will be redelivered or sent to dead letter queue).
    fn nack(&self, event_id: &str, reason: &str) -> Result<(), PublishError>;
}

/// Trait for subscribers that can create independent subscriber instances.
///
/// This enables filtered subscriptions via `Bus::subscribe()`.
pub trait Subscribable: Subscriber + Sized {
    /// Create a new independent subscriber sharing the same event source.
    ///
    /// The new subscriber has its own read position, allowing multiple
    /// independent consumers of the same event stream.
    fn new_subscriber(&self) -> Self;
}

/// Combined trait for bidirectional bus communication.
pub trait EventBus: Publisher + Subscriber {}

// Blanket implementation
impl<T: Publisher + Subscriber> EventBus for T {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_construction() {
        let event = Event::new("evt-1", "OrderCreated", b"{}".to_vec());
        assert_eq!(event.id, "evt-1");
        assert_eq!(event.event_type, "OrderCreated");
        assert_eq!(event.payload_str(), Some("{}"));
    }

    #[test]
    fn event_with_metadata() {
        let event = Event::new("evt-1", "OrderCreated", b"{}".to_vec())
            .with_metadata("correlation-id", "abc-123")
            .with_metadata("source", "order-service");

        let meta = event.metadata.unwrap();
        assert_eq!(meta.len(), 2);
        assert_eq!(meta[0], ("correlation-id".to_string(), "abc-123".to_string()));
    }
}
