//! The canonical transport message vocabulary: [`Message`] + [`MessageKind`].
//!
//! Bus-core types with no dependency on `microsvc`: a `Message` carries an
//! optional durable id, a name, a [`MessageKind`] (command vs event), the raw
//! payload, a content type, and metadata. Payload decoding returns a bus-core
//! [`PayloadDecodeError`] (not `microsvc::HandlerError`), so the bus does not
//! depend up into microsvc; microsvc maps it back via `From`.

/// The kind of message a handler consumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize)]
pub enum MessageKind {
    /// A command addressed to one handler.
    Command,
    /// A published event that may be consumed by many handlers.
    Event,
}

impl MessageKind {
    /// The wire token transports use to carry the kind in headers/metadata.
    pub fn as_str(&self) -> &'static str {
        match self {
            MessageKind::Command => "command",
            MessageKind::Event => "event",
        }
    }

    /// Parse a wire token back into a kind, defaulting to [`MessageKind::Event`]
    /// for anything that is not exactly `"command"`.
    ///
    /// The bias toward `Event` is deliberate: every transport that round-trips
    /// the kind already did so, and treating an unrecognized token as a fan-out
    /// event is the safe default for delivery.
    pub fn from_str_lossy(value: &str) -> MessageKind {
        match value {
            "command" => MessageKind::Command,
            _ => MessageKind::Event,
        }
    }
}

/// Transport subscription metadata derived from a router's registered handlers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubscriptionPlan {
    /// Command names consumed by point-to-point command transports.
    pub commands: Vec<String>,
    /// Event names consumed by fan-out event transports.
    pub events: Vec<String>,
}

/// Failure decoding a [`Message`] payload.
///
/// A bus-core error so [`Message`] stays free of `microsvc::HandlerError`. The
/// microsvc side provides `From<PayloadDecodeError> for HandlerError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayloadDecodeError(pub String);

impl std::fmt::Display for PayloadDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for PayloadDecodeError {}

/// Serializable transport message used by handlers.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct Message {
    pub id: Option<String>,
    pub name: String,
    pub kind: MessageKind,
    pub payload: Vec<u8>,
    pub content_type: String,
    pub metadata: Vec<(String, String)>,
}

impl Message {
    /// Create a transport message.
    pub fn new(name: impl Into<String>, kind: MessageKind, payload: Vec<u8>) -> Self {
        Self {
            id: None,
            name: name.into(),
            kind,
            payload,
            content_type: "application/json".to_string(),
            metadata: Vec::new(),
        }
    }

    /// Add a durable message id.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Add metadata.
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.push((key.into(), value.into()));
        self
    }

    /// Get the durable message id, if this message has one.
    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    /// Get the message name.
    ///
    /// The name flows unmodified into broker routing primitives (NATS subject
    /// suffix, Kafka topic, RabbitMQ routing/binding key). A `.` is a routing
    /// *segment separator* on topic transports, so it changes routing
    /// granularity rather than being an opaque label — see
    /// [`validate_message_name`](super::validate_message_name).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Validate this message's name for use as a transport routing identifier.
    ///
    /// Delegates to [`validate_message_name`](super::validate_message_name):
    /// rejects empty, over-long, control-character-bearing, or wildcard-bearing
    /// (`*`/`#`/`>`) names. Publish boundaries call this before a name becomes a
    /// subject/topic/routing key; the inbound Knative ingress calls it on the
    /// `ce-type` it reads off the wire.
    pub fn validate_name(&self) -> Result<(), super::MessageNameError> {
        super::validate_message_name(&self.name).map(|_| ())
    }

    /// Get the raw payload bytes.
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Get a metadata value by key.
    pub fn metadata(&self, key: &str) -> Option<&str> {
        self.metadata
            .iter()
            .find(|(existing, _)| existing.eq_ignore_ascii_case(key))
            .map(|(_, value)| value.as_str())
    }

    /// Get the correlation id, if present.
    pub fn correlation_id(&self) -> Option<&str> {
        self.metadata("correlation_id")
    }

    /// Get the causation id, if present.
    pub fn causation_id(&self) -> Option<&str> {
        self.metadata("causation_id")
    }

    /// Decode the raw payload as JSON.
    pub fn payload_json<T: serde::de::DeserializeOwned>(&self) -> Result<T, PayloadDecodeError> {
        serde_json::from_slice(&self.payload).map_err(|e| {
            PayloadDecodeError(format!(
                "invalid JSON payload for message '{}': {}",
                self.name, e
            ))
        })
    }

    /// Decode the raw payload as bitcode.
    ///
    /// Security caveat: bitcode is a compact binary format that is **not hardened
    /// against hostile input**. Only decode bitcode from trusted producers — do
    /// not use it for payloads arriving on a public bus or other untrusted
    /// ingress, where a malformed/adversarial buffer could trigger excessive
    /// allocation or a panic. Prefer [`payload_json`](Self::payload_json) for
    /// untrusted input.
    pub fn payload_bitcode<T: serde::de::DeserializeOwned>(&self) -> Result<T, PayloadDecodeError> {
        bitcode::deserialize(&self.payload).map_err(|e| {
            PayloadDecodeError(format!(
                "invalid bitcode payload for message '{}': {}",
                self.name, e
            ))
        })
    }
}

/// Derive a message name from a transport address (NATS subject, Kafka topic,
/// or AMQP routing key) by stripping the publisher's `prefix`.
///
/// Takes ownership of `address` so the common `None`/no-match paths reuse the
/// existing allocation instead of cloning. When `prefix` is set but does not
/// match, the full address is returned unchanged.
#[cfg(any(feature = "nats", feature = "kafka", feature = "rabbitmq"))]
pub(crate) fn strip_address_prefix(address: String, prefix: Option<&str>) -> String {
    match prefix {
        Some(prefix) => address
            .strip_prefix(prefix)
            .map(str::to_string)
            .unwrap_or(address),
        None => address,
    }
}
