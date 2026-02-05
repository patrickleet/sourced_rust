use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::core::Entity;
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
        message.initialize(id.into(), event_type.into(), payload);
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
    pub fn initialize(&mut self, id: String, event_type: String, payload: Vec<u8>) {
        let normalized_id = Self::normalize_id(id);
        self.entity.set_id(&normalized_id);
        self.event_type = event_type;
        self.payload = payload;
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
    "MessageCreated"(id, event_type, payload) => initialize,
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
        );
        assert_eq!(message.event_type, "UserCreated");
        assert!(message.is_pending());
    }

    #[test]
    fn claim_and_complete() {
        let mut message = OutboxMessage::new();
        message.initialize("msg-1".into(), "Event1".into(), b"{}".to_vec());
        message.claim_for("worker-1", Duration::from_secs(60));
        assert!(message.is_in_flight());
        assert_eq!(message.attempts, 1);

        message.complete();
        assert!(message.is_published());
    }

    #[test]
    fn release_and_fail() {
        let mut message = OutboxMessage::new();
        message.initialize("msg-1".into(), "Event1".into(), b"{}".to_vec());
        message.claim_for("worker-1", Duration::from_secs(60));

        message.release("timeout".into());
        assert!(message.is_pending());
        assert_eq!(message.last_error.as_deref(), Some("timeout"));

        message.claim_for("worker-1", Duration::from_secs(60));
        message.fail("max retries".into());
        assert!(message.is_failed());
    }
}
