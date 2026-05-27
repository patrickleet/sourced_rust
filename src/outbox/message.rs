use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::aggregate::Aggregate;
use crate::entity::{BitcodePayloadCodec, Entity, EventRecordError, PayloadCodec};
use crate::SourcedResult;

/// Status of an outbox message.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutboxMessageStatus {
    #[default]
    Pending,
    InFlight,
    Published,
    Failed,
}

impl OutboxMessageStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InFlight => "in_flight",
            Self::Published => "published",
            Self::Failed => "failed",
        }
    }
}

impl std::str::FromStr for OutboxMessageStatus {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "pending" => Ok(Self::Pending),
            "in_flight" => Ok(Self::InFlight),
            "published" => Ok(Self::Published),
            "failed" => Ok(Self::Failed),
            _ => Err(()),
        }
    }
}

/// Durable publication work item stored through the outbox pattern.
///
/// The message is an immutable publishable envelope plus mutable delivery state.
/// It is not an aggregate stream; repositories store it in their outbox storage
/// and workers update delivery state directly.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OutboxMessage {
    pub id: String,
    pub event_type: String,
    pub payload: Vec<u8>,
    pub payload_codec: String,
    pub payload_codec_version: u16,
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
    pub metadata: HashMap<String, String>,
    pub source_aggregate_type: Option<String>,
    pub source_aggregate_id: Option<String>,
    pub source_sequence: Option<u64>,
}

impl Default for OutboxMessage {
    fn default() -> Self {
        Self {
            id: String::new(),
            event_type: String::new(),
            payload: Vec::new(),
            payload_codec: OutboxMessage::RAW_PAYLOAD_CODEC.to_string(),
            payload_codec_version: OutboxMessage::RAW_PAYLOAD_CODEC_VERSION,
            status: OutboxMessageStatus::default(),
            created_at: SystemTime::UNIX_EPOCH,
            attempts: 0,
            last_error: None,
            worker_id: None,
            leased_until: None,
            destination: None,
            metadata: HashMap::new(),
            source_aggregate_type: None,
            source_aggregate_id: None,
            source_sequence: None,
        }
    }
}

impl OutboxMessage {
    pub const RAW_PAYLOAD_CODEC: &'static str = "bytes";
    pub const RAW_PAYLOAD_CODEC_VERSION: u16 = 1;

    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new outbox message with raw bytes payload.
    ///
    /// The `event_type` names the publishable message, not an aggregate replay
    /// record unless the caller intentionally uses the same name.
    pub fn create(
        id: impl Into<String>,
        event_type: impl Into<String>,
        payload: Vec<u8>,
    ) -> SourcedResult<Self> {
        let mut message = Self::new();
        message.initialize(id.into(), event_type.into(), payload, None, HashMap::new())?;
        Ok(message)
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
    ) -> SourcedResult<Self> {
        let mut message = Self::new();
        message.initialize(
            id.into(),
            event_type.into(),
            payload,
            Some(destination.into()),
            HashMap::new(),
        )?;
        Ok(message)
    }

    /// Create a new outbox message with bitcode (fast binary) serialization.
    pub fn encode<T: Serialize>(
        id: impl Into<String>,
        event_type: impl Into<String>,
        payload: &T,
    ) -> SourcedResult<Self> {
        let bytes = BitcodePayloadCodec::encode(payload).map_err(EventRecordError::encode)?;
        Self::create_encoded_bytes(id, event_type, bytes, None, HashMap::new())
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
    ) -> SourcedResult<Self> {
        let bytes = BitcodePayloadCodec::encode(payload).map_err(EventRecordError::encode)?;
        Self::create_encoded_bytes(
            id,
            event_type,
            bytes,
            Some(destination.into()),
            HashMap::new(),
        )
    }

    /// Create a message with metadata and raw bytes payload.
    pub fn create_with_metadata(
        id: impl Into<String>,
        event_type: impl Into<String>,
        payload: Vec<u8>,
        metadata: HashMap<String, String>,
    ) -> SourcedResult<Self> {
        let mut message = Self::new();
        message.initialize(id.into(), event_type.into(), payload, None, metadata)?;
        Ok(message)
    }

    /// Create a message with metadata and bitcode-serialized payload.
    pub fn encode_with_metadata<T: Serialize>(
        id: impl Into<String>,
        event_type: impl Into<String>,
        payload: &T,
        metadata: HashMap<String, String>,
    ) -> SourcedResult<Self> {
        let bytes = BitcodePayloadCodec::encode(payload).map_err(EventRecordError::encode)?;
        Self::create_encoded_bytes(id, event_type, bytes, None, metadata)
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
    ) -> SourcedResult<Self> {
        let bytes = BitcodePayloadCodec::encode(payload).map_err(EventRecordError::encode)?;
        Self::create_encoded_bytes(id, event_type, bytes, None, entity.metadata().clone())
    }

    /// Create a domain-event style message from a `Snapshottable` aggregate.
    ///
    /// This is the recommended way to publish an aggregate-derived fact. It
    /// derives everything from the aggregate automatically:
    /// - **id**: `"{entity_id}:{event_type}:{version}"`
    /// - **payload**: the aggregate's snapshot (via `create_snapshot()`)
    /// - **metadata**: propagated from the entity (correlation IDs, trace context, etc.)
    ///
    /// ```ignore
    /// let outbox = OutboxMessage::domain_event("TodoInitialized", &todo)?;
    /// repo.outbox_sync(outbox).commit_sync(&mut todo)?;
    /// ```
    pub fn domain_event<A: crate::Snapshottable>(
        event_type: impl Into<String>,
        aggregate: &A,
    ) -> SourcedResult<Self> {
        let event_type = event_type.into();
        let entity = aggregate.entity();
        let id = format!("{}:{}:{}", entity.id(), event_type, entity.version());
        let snapshot = aggregate.create_snapshot();
        let bytes = BitcodePayloadCodec::encode(&snapshot).map_err(EventRecordError::encode)?;
        let mut message =
            Self::create_encoded_bytes(id, event_type, bytes, None, entity.metadata().clone())?;
        message.set_source(aggregate);
        Ok(message)
    }

    /// Decode the payload from the default binary codec.
    pub fn decode<T: serde::de::DeserializeOwned>(&self) -> SourcedResult<T> {
        if self.payload_codec != BitcodePayloadCodec::NAME
            || self.payload_codec_version != BitcodePayloadCodec::VERSION
        {
            return Err(EventRecordError::unsupported_codec(
                &self.payload_codec,
                self.payload_codec_version,
            ));
        }

        BitcodePayloadCodec::decode(&self.payload).map_err(|e| {
            EventRecordError::decode(
                &self.event_type,
                &self.payload_codec,
                self.payload_codec_version,
                e,
            )
        })
    }

    // Getters
    pub fn id(&self) -> &str {
        &self.id
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

    pub fn has_expired_lease_at(&self, now: SystemTime) -> bool {
        self.is_in_flight() && self.leased_until.map(|until| until <= now).unwrap_or(true)
    }

    pub fn is_claimable_at(&self, now: SystemTime) -> bool {
        self.is_pending() || self.has_expired_lease_at(now)
    }

    pub fn is_claimed_by(&self, worker_id: &str) -> bool {
        self.worker_id.as_deref() == Some(worker_id)
    }

    fn is_claimable(&self) -> bool {
        self.is_claimable_at(SystemTime::now())
    }

    // Commands
    pub fn initialize(
        &mut self,
        id: String,
        event_type: String,
        payload: Vec<u8>,
        destination: Option<String>,
        metadata: HashMap<String, String>,
    ) -> SourcedResult {
        self.initialize_with_codec(
            id,
            event_type,
            payload,
            destination,
            metadata,
            (
                Self::RAW_PAYLOAD_CODEC.to_string(),
                Self::RAW_PAYLOAD_CODEC_VERSION,
            ),
        )
    }

    fn initialize_with_codec(
        &mut self,
        id: String,
        event_type: String,
        payload: Vec<u8>,
        destination: Option<String>,
        metadata: HashMap<String, String>,
        payload_codec: (String, u16),
    ) -> SourcedResult {
        validate_non_empty("outbox message id", &id)?;
        validate_non_empty("outbox event type", &event_type)?;
        validate_non_empty("outbox payload codec", &payload_codec.0)?;
        if payload_codec.1 == 0 {
            return Err(EventRecordError {
                message: "outbox payload codec version must be greater than zero".into(),
            });
        }
        self.id = id;
        self.event_type = event_type;
        self.payload = payload;
        self.payload_codec = payload_codec.0;
        self.payload_codec_version = payload_codec.1;
        self.destination = destination;
        self.metadata = metadata;
        self.status = OutboxMessageStatus::Pending;
        self.created_at = SystemTime::now();
        self.attempts = 0;
        self.last_error = None;
        self.worker_id = None;
        self.leased_until = None;
        self.source_aggregate_type = None;
        self.source_aggregate_id = None;
        self.source_sequence = None;
        Ok(())
    }

    fn create_encoded_bytes(
        id: impl Into<String>,
        event_type: impl Into<String>,
        payload: Vec<u8>,
        destination: Option<String>,
        metadata: HashMap<String, String>,
    ) -> SourcedResult<Self> {
        let mut message = Self::new();
        message.initialize_with_codec(
            id.into(),
            event_type.into(),
            payload,
            destination,
            metadata,
            (
                BitcodePayloadCodec::NAME.to_string(),
                BitcodePayloadCodec::VERSION,
            ),
        )?;
        Ok(message)
    }

    pub fn claim(&mut self, worker_id: String, until_secs: u64) -> SourcedResult {
        if !self.is_claimable() {
            return Err(EventRecordError {
                message: format!("outbox message `{}` is not claimable", self.id),
            });
        }
        validate_non_empty("outbox worker id", &worker_id)?;

        let until_time = SystemTime::UNIX_EPOCH + Duration::from_secs(until_secs);
        self.status = OutboxMessageStatus::InFlight;
        self.attempts += 1;
        self.worker_id = Some(worker_id);
        self.leased_until = Some(until_time);
        Ok(())
    }

    /// Claim with a Duration (convenience method that computes until_secs)
    pub fn claim_for(&mut self, worker_id: impl Into<String>, lease: Duration) -> SourcedResult {
        self.claim_at(worker_id, lease, SystemTime::now())
    }

    /// Claim with an explicit clock value. This is useful for deterministic
    /// tests and repository implementations that capture time once per batch.
    pub fn claim_at(
        &mut self,
        worker_id: impl Into<String>,
        lease: Duration,
        now: SystemTime,
    ) -> SourcedResult {
        let until_secs = Self::lease_deadline_secs(now, lease)?;
        self.claim(worker_id.into(), until_secs)
    }

    fn lease_deadline_secs(now: SystemTime, lease: Duration) -> SourcedResult<u64> {
        let until = now.checked_add(lease).ok_or_else(|| EventRecordError {
            message: "failed to compute outbox lease deadline: timestamp overflow".into(),
        })?;

        let until_secs = until
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_err(|err| EventRecordError {
                message: format!(
                    "failed to compute outbox lease deadline before UNIX epoch: {}",
                    err
                ),
            })?
            .as_secs();

        Ok(until_secs)
    }

    pub fn complete(&mut self) -> SourcedResult {
        if !self.is_in_flight() {
            return Err(EventRecordError {
                message: format!("outbox message `{}` is not in flight", self.id),
            });
        }
        self.status = OutboxMessageStatus::Published;
        self.worker_id = None;
        self.leased_until = None;
        Ok(())
    }

    pub fn release(&mut self, error: String) -> SourcedResult {
        if !self.is_in_flight() {
            return Err(EventRecordError {
                message: format!("outbox message `{}` is not in flight", self.id),
            });
        }
        self.status = OutboxMessageStatus::Pending;
        self.last_error = if error.is_empty() { None } else { Some(error) };
        self.worker_id = None;
        self.leased_until = None;
        Ok(())
    }

    pub fn fail(&mut self, error: String) -> SourcedResult {
        if !self.can_fail() {
            return Err(EventRecordError {
                message: format!("outbox message `{}` cannot be failed", self.id),
            });
        }
        self.status = OutboxMessageStatus::Failed;
        self.last_error = if error.is_empty() { None } else { Some(error) };
        self.worker_id = None;
        self.leased_until = None;
        Ok(())
    }

    fn can_fail(&self) -> bool {
        self.status != OutboxMessageStatus::Published && self.status != OutboxMessageStatus::Failed
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

    pub fn set_source<A: Aggregate>(&mut self, aggregate: &A) {
        self.source_aggregate_type = Some(A::aggregate_type().to_string());
        self.source_aggregate_id = Some(aggregate.entity().id().to_string());
        self.source_sequence = Some(aggregate.entity().version());
    }
}

fn validate_non_empty(field: &str, value: &str) -> SourcedResult {
    if value.trim().is_empty() {
        return Err(EventRecordError {
            message: format!("{field} must not be empty"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_message_is_pending() {
        let mut message = OutboxMessage::new();
        message
            .initialize(
                "msg-1".into(),
                "UserCreated".into(),
                br#"{"id":"123"}"#.to_vec(),
                None,
                HashMap::new(),
            )
            .unwrap();
        assert_eq!(message.event_type, "UserCreated");
        assert!(message.is_pending());
    }

    #[test]
    fn claim_and_complete() {
        let mut message = OutboxMessage::new();
        message
            .initialize(
                "msg-1".into(),
                "Event1".into(),
                b"{}".to_vec(),
                None,
                HashMap::new(),
            )
            .unwrap();
        message
            .claim_for("worker-1", Duration::from_secs(60))
            .unwrap();
        assert!(message.is_in_flight());
        assert_eq!(message.attempts, 1);

        message.complete().unwrap();
        assert!(message.is_published());
    }

    #[test]
    fn expired_in_flight_message_can_be_claimed_again() {
        let mut message = OutboxMessage::create("msg-1", "Event", b"{}".to_vec()).unwrap();
        message
            .claim_at("worker-1", Duration::from_secs(1), SystemTime::UNIX_EPOCH)
            .unwrap();

        assert!(message.has_expired_lease_at(SystemTime::now()));
        assert!(message.is_claimable_at(SystemTime::now()));

        message
            .claim_for("worker-2", Duration::from_secs(60))
            .unwrap();

        assert_eq!(message.worker_id.as_deref(), Some("worker-2"));
        assert_eq!(message.attempts, 2);
    }

    #[test]
    fn claim_deadline_overflow_returns_error() {
        let err = OutboxMessage::lease_deadline_secs(
            SystemTime::UNIX_EPOCH,
            Duration::from_secs(u64::MAX),
        )
        .unwrap_err();

        assert!(err
            .message
            .contains("failed to compute outbox lease deadline"));
    }

    #[test]
    fn claim_deadline_before_epoch_returns_error() {
        let before_epoch = SystemTime::UNIX_EPOCH
            .checked_sub(Duration::from_secs(1))
            .unwrap();
        let err = OutboxMessage::lease_deadline_secs(before_epoch, Duration::ZERO).unwrap_err();

        assert!(err
            .message
            .contains("failed to compute outbox lease deadline before UNIX epoch"));
    }

    #[test]
    fn initialize_resets_delivery_and_source_state() {
        let mut message = OutboxMessage::create("msg-1", "Event", b"{}".to_vec()).unwrap();
        message.claim("worker-1".into(), 1).unwrap();
        message.last_error = Some("previous failure".into());
        message.source_aggregate_type = Some("todo".into());
        message.source_aggregate_id = Some("todo-1".into());
        message.source_sequence = Some(7);

        message
            .initialize(
                "msg-2".into(),
                "OtherEvent".into(),
                b"{}".to_vec(),
                None,
                HashMap::new(),
            )
            .unwrap();

        assert_eq!(message.attempts, 0);
        assert_eq!(message.last_error, None);
        assert_eq!(message.worker_id, None);
        assert_eq!(message.leased_until, None);
        assert_eq!(message.source_aggregate_type, None);
        assert_eq!(message.source_aggregate_id, None);
        assert_eq!(message.source_sequence, None);
    }

    #[test]
    fn create_with_metadata() {
        let mut meta = HashMap::new();
        meta.insert("correlation_id".to_string(), "req-abc".to_string());
        meta.insert("trace_id".to_string(), "t-999".to_string());

        let message =
            OutboxMessage::create_with_metadata("msg-1", "UserCreated", b"{}".to_vec(), meta)
                .unwrap();
        assert_eq!(message.correlation_id(), Some("req-abc"));
        assert_eq!(message.meta("trace_id"), Some("t-999"));
    }

    #[test]
    fn encode_with_metadata() {
        let mut meta = HashMap::new();
        meta.insert("correlation_id".to_string(), "req-456".to_string());

        let payload = ("hello", 42i32);
        let message =
            OutboxMessage::encode_with_metadata("msg-2", "SomeEvent", &payload, meta).unwrap();

        assert_eq!(message.correlation_id(), Some("req-456"));
        let decoded: (String, i32) = message.decode().unwrap();
        assert_eq!(decoded, ("hello".to_string(), 42));
    }

    #[test]
    fn set_metadata_individually() {
        let mut message = OutboxMessage::create("msg-1", "Event", b"{}".to_vec()).unwrap();
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
        message
            .initialize(
                "msg-1".into(),
                "Event1".into(),
                b"{}".to_vec(),
                None,
                HashMap::new(),
            )
            .unwrap();
        message
            .claim_for("worker-1", Duration::from_secs(60))
            .unwrap();

        message.release("timeout".into()).unwrap();
        assert!(message.is_pending());
        assert_eq!(message.last_error.as_deref(), Some("timeout"));

        message
            .claim_for("worker-1", Duration::from_secs(60))
            .unwrap();
        message.fail("max retries".into()).unwrap();
        assert!(message.is_failed());
    }
}
