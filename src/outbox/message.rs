use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::entity::Entity;
use crate::digest;

/// Status of an outbox message.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutboxMessageStatus {
    #[default]
    Pending,
    InFlight,
    Published,
    Failed,
}

/// Event-sourced outbox message.
#[derive(Debug, Serialize, Deserialize)]
pub struct OutboxMessage {
    pub entity: Entity,
    pub event_type: String,
    pub payload: Vec<u8>,
    pub status: OutboxMessageStatus,
    pub created_at: SystemTime,
    pub attempts: u32,
    pub last_error: Option<String>,
    pub worker_id: Option<String>,
    pub leased_until: Option<SystemTime>,
    /// Optional destination queue for point-to-point delivery via `send/listen`.
    /// When set, the outbox worker uses `Sender::send(destination, event)` instead
    /// of `Publisher::publish(event)`.
    pub destination: Option<String>,
    /// Metadata propagated to the publisher (correlation IDs, trace context, etc.).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
}

impl Default for OutboxMessage {
    fn default() -> Self {
        Self {
            entity: Entity::default(),
            event_type: String::new(),
            payload: Vec::new(),
            status: OutboxMessageStatus::default(),
            created_at: SystemTime::UNIX_EPOCH,
            attempts: 0,
            last_error: None,
            worker_id: None,
            leased_until: None,
            destination: None,
            metadata: HashMap::new(),
        }
    }
}


impl OutboxMessage {
    pub const ID_PREFIX: &'static str = "outbox:";

    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new outbox message with raw bytes payload.
    pub fn create(
        id: impl Into<String>,
        event_type: impl Into<String>,
        payload: Vec<u8>,
    ) -> Self {
        let mut message = Self::new();
        message.initialize(id.into(), event_type.into(), payload, None, HashMap::new());
        message
    }

    /// Create a new outbox message with raw bytes payload and a destination queue.
    ///
    /// When a destination is set, the outbox worker uses `Sender::send(destination, event)`
    /// (point-to-point) instead of `Publisher::publish(event)` (fan-out).
    pub fn create_to(
        id: impl Into<String>,
        event_type: impl Into<String>,
        destination: impl Into<String>,
        payload: Vec<u8>,
    ) -> Self {
        let mut message = Self::new();
        message.initialize(id.into(), event_type.into(), payload, Some(destination.into()), HashMap::new());
        message
    }

    /// Create a new outbox message with bitcode (fast binary) serialization.
    pub fn encode<T: Serialize>(
        id: impl Into<String>,
        event_type: impl Into<String>,
        payload: &T,
    ) -> Result<Self, bitcode::Error> {
        let bytes = bitcode::serialize(payload)?;
        Ok(Self::create(id, event_type, bytes))
    }

    /// Create a new outbox message with bitcode serialization and a destination queue.
    ///
    /// When a destination is set, the outbox worker uses `Sender::send(destination, event)`
    /// (point-to-point) instead of `Publisher::publish(event)` (fan-out).
    pub fn encode_to<T: Serialize>(
        id: impl Into<String>,
        event_type: impl Into<String>,
        destination: impl Into<String>,
        payload: &T,
    ) -> Result<Self, bitcode::Error> {
        let bytes = bitcode::serialize(payload)?;
        Ok(Self::create_to(id, event_type, destination, bytes))
    }

    /// Create a message with metadata and raw bytes payload.
    pub fn create_with_metadata(
        id: impl Into<String>,
        event_type: impl Into<String>,
        payload: Vec<u8>,
        metadata: HashMap<String, String>,
    ) -> Self {
        let mut message = Self::new();
        message.initialize(id.into(), event_type.into(), payload, None, metadata);
        message
    }

    /// Create a message with metadata and bitcode-serialized payload.
    pub fn encode_with_metadata<T: Serialize>(
        id: impl Into<String>,
        event_type: impl Into<String>,
        payload: &T,
        metadata: HashMap<String, String>,
    ) -> Result<Self, bitcode::Error> {
        let bytes = bitcode::serialize(payload)?;
        Ok(Self::create_with_metadata(id, event_type, bytes, metadata))
    }

    /// Create a message that inherits metadata from an entity's context.
    ///
    /// This automatically propagates correlation IDs, trace context, and any
    /// other metadata set on the entity, so nothing gets lost between the
    /// event store and the bus.
    pub fn encode_for_entity<T: Serialize>(
        id: impl Into<String>,
        event_type: impl Into<String>,
        payload: &T,
        entity: &Entity,
    ) -> Result<Self, bitcode::Error> {
        let bytes = bitcode::serialize(payload)?;
        Ok(Self::create_with_metadata(id, event_type, bytes, entity.metadata().clone()))
    }

    /// Create a message from a `Snapshottable` aggregate.
    ///
    /// This is the recommended way to create outbox messages — it derives
    /// everything from the aggregate automatically:
    /// - **id**: `"{entity_id}:{event_type}:{version}"`
    /// - **payload**: the aggregate's snapshot (via `create_snapshot()`)
    /// - **metadata**: propagated from the entity (correlation IDs, trace context, etc.)
    ///
    /// ```ignore
    /// let mut outbox = OutboxMessage::domain_event("TodoInitialized", &todo)?;
    /// repo.outbox(&mut outbox).commit(&mut todo)?;
    /// ```
    pub fn domain_event<A: crate::Snapshottable>(
        event_type: impl Into<String>,
        aggregate: &A,
    ) -> Result<Self, bitcode::Error> {
        let event_type = event_type.into();
        let entity = aggregate.entity();
        let id = format!("{}:{}:{}", entity.id(), event_type, entity.version());
        let snapshot = aggregate.create_snapshot();
        let bytes = bitcode::serialize(&snapshot)?;
        Ok(Self::create_with_metadata(id, event_type, bytes, entity.metadata().clone()))
    }

    /// Decode the payload from bitcode binary format.
    pub fn decode<T: serde::de::DeserializeOwned>(&self) -> Result<T, bitcode::Error> {
        bitcode::deserialize(&self.payload)
    }

    // Getters
    pub fn id(&self) -> &str {
        self.entity.id()
    }

    pub fn payload_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.payload).ok()
    }

    pub fn is_pending(&self) -> bool {
        self.status == OutboxMessageStatus::Pending
    }

    pub fn is_in_flight(&self) -> bool {
        self.status == OutboxMessageStatus::InFlight
    }

    pub fn is_published(&self) -> bool {
        self.status == OutboxMessageStatus::Published
    }

    pub fn is_failed(&self) -> bool {
        self.status == OutboxMessageStatus::Failed
    }

    // Commands
    #[digest("MessageCreated")]
    pub fn initialize(
        &mut self,
        id: String,
        event_type: String,
        payload: Vec<u8>,
        destination: Option<String>,
        metadata: HashMap<String, String>,
    ) {
        let normalized_id = Self::normalize_id(id);
        self.entity.set_id(&normalized_id);
        self.event_type = event_type;
        self.payload = payload;
        self.destination = destination;
        self.metadata = metadata;
        self.status = OutboxMessageStatus::Pending;
        self.created_at = SystemTime::now();
    }

    #[digest("MessageClaimed", when = self.is_pending())]
    pub fn claim(&mut self, worker_id: String, until_secs: u64) {
        let until_time = SystemTime::UNIX_EPOCH + Duration::from_secs(until_secs);
        self.status = OutboxMessageStatus::InFlight;
        self.attempts += 1;
        self.worker_id = Some(worker_id);
        self.leased_until = Some(until_time);
    }

    /// Claim with a Duration (convenience method that computes until_secs)
    pub fn claim_for(&mut self, worker_id: impl Into<String>, lease: Duration) {
        let now = SystemTime::now();
        let until = now + lease;
        let until_secs = until
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.claim(worker_id.into(), until_secs);
    }

    #[digest("MessagePublished", when = self.is_in_flight())]
    pub fn complete(&mut self) {
        self.status = OutboxMessageStatus::Published;
        self.worker_id = None;
        self.leased_until = None;
    }

    #[digest("MessageReleased", when = self.is_in_flight())]
    pub fn release(&mut self, error: String) {
        self.status = OutboxMessageStatus::Pending;
        self.last_error = if error.is_empty() { None } else { Some(error) };
        self.worker_id = None;
        self.leased_until = None;
    }

    #[digest("MessageFailed", when = self.can_fail())]
    pub fn fail(&mut self, error: String) {
        self.status = OutboxMessageStatus::Failed;
        self.last_error = if error.is_empty() { None } else { Some(error) };
        self.worker_id = None;
        self.leased_until = None;
    }

    fn can_fail(&self) -> bool {
        self.status != OutboxMessageStatus::Published && self.status != OutboxMessageStatus::Failed
    }

    fn normalize_id(id: impl Into<String>) -> String {
        let id = id.into();
        if id.starts_with(Self::ID_PREFIX) {
            id
        } else {
            format!("{}{}", Self::ID_PREFIX, id)
        }
    }

    /// Set a single metadata key-value pair.
    pub fn set_meta(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.metadata.insert(key.into(), value.into());
    }

    /// Set the correlation ID.
    pub fn set_correlation_id(&mut self, id: impl Into<String>) {
        self.set_meta("correlation_id", id);
    }

    /// Set the causation ID.
    pub fn set_causation_id(&mut self, id: impl Into<String>) {
        self.set_meta("causation_id", id);
    }

    /// Get a metadata value by key.
    pub fn meta(&self, key: &str) -> Option<&str> {
        self.metadata.get(key).map(|s| s.as_str())
    }

    /// Get the correlation ID, if set.
    pub fn correlation_id(&self) -> Option<&str> {
        self.meta("correlation_id")
    }

    /// Get the causation ID, if set.
    pub fn causation_id(&self) -> Option<&str> {
        self.meta("causation_id")
    }

    /// Get mutable reference to the underlying entity.
    pub fn entity_mut(&mut self) -> &mut Entity {
        &mut self.entity
    }

    /// Consume the message and return the underlying entity.
    pub fn into_entity(self) -> Entity {
        self.entity
    }
}

crate::aggregate!(OutboxMessage, entity {
    "MessageCreated"(id, event_type, payload, destination, metadata) => initialize,
    "MessageClaimed"(worker_id, until_secs) => claim,
    "MessagePublished"() => complete,
    "MessageReleased"(error) => release,
    "MessageFailed"(error) => fail,
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_message_is_pending() {
        let mut message = OutboxMessage::new();
        message.initialize(
            "msg-1".into(),
            "UserCreated".into(),
            br#"{"id":"123"}"#.to_vec(),
            None,
            HashMap::new(),
        );
        assert_eq!(message.event_type, "UserCreated");
        assert!(message.is_pending());
    }

    #[test]
    fn claim_and_complete() {
        let mut message = OutboxMessage::new();
        message.initialize("msg-1".into(), "Event1".into(), b"{}".to_vec(), None, HashMap::new());
        message.claim_for("worker-1", Duration::from_secs(60));
        assert!(message.is_in_flight());
        assert_eq!(message.attempts, 1);

        message.complete();
        assert!(message.is_published());
    }

    #[test]
    fn create_with_metadata() {
        let mut meta = HashMap::new();
        meta.insert("correlation_id".to_string(), "req-abc".to_string());
        meta.insert("trace_id".to_string(), "t-999".to_string());

        let message = OutboxMessage::create_with_metadata(
            "msg-1",
            "UserCreated",
            b"{}".to_vec(),
            meta,
        );
        assert_eq!(message.correlation_id(), Some("req-abc"));
        assert_eq!(message.meta("trace_id"), Some("t-999"));
    }

    #[test]
    fn encode_with_metadata() {
        let mut meta = HashMap::new();
        meta.insert("correlation_id".to_string(), "req-456".to_string());

        let payload = ("hello", 42i32);
        let message = OutboxMessage::encode_with_metadata(
            "msg-2",
            "SomeEvent",
            &payload,
            meta,
        )
        .unwrap();

        assert_eq!(message.correlation_id(), Some("req-456"));
        let decoded: (String, i32) = message.decode().unwrap();
        assert_eq!(decoded, ("hello".to_string(), 42));
    }

    #[test]
    fn set_metadata_individually() {
        let mut message = OutboxMessage::create("msg-1", "Event", b"{}".to_vec());
        message.set_correlation_id("req-abc");
        message.set_causation_id("evt-prior");
        message.set_meta("tenant", "acme");

        assert_eq!(message.correlation_id(), Some("req-abc"));
        assert_eq!(message.causation_id(), Some("evt-prior"));
        assert_eq!(message.meta("tenant"), Some("acme"));
    }

    #[test]
    fn release_and_fail() {
        let mut message = OutboxMessage::new();
        message.initialize("msg-1".into(), "Event1".into(), b"{}".to_vec(), None, HashMap::new());
        message.claim_for("worker-1", Duration::from_secs(60));

        message.release("timeout".into());
        assert!(message.is_pending());
        assert_eq!(message.last_error.as_deref(), Some("timeout"));

        message.claim_for("worker-1", Duration::from_secs(60));
        message.fail("max retries".into());
        assert!(message.is_failed());
    }
}
