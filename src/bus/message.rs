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
    pub fn name(&self) -> &str {
        &self.name
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
    pub fn payload_bitcode<T: serde::de::DeserializeOwned>(&self) -> Result<T, PayloadDecodeError> {
        bitcode::deserialize(&self.payload).map_err(|e| {
            PayloadDecodeError(format!(
                "invalid bitcode payload for message '{}': {}",
                self.name, e
            ))
        })
    }
}
