use std::collections::HashMap;
use std::fmt;
use std::time::SystemTime;
use serde::{Serialize, Deserialize, de::DeserializeOwned};

/// Error when deserializing event payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PayloadError {
    pub message: String,
}

impl fmt::Display for PayloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "payload error: {}", self.message)
    }
}

impl std::error::Error for PayloadError {}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct EventRecord {
    pub event_name: String,
    #[serde(with = "payload_serde")]
    pub payload: Vec<u8>,
    pub sequence: u64,
    pub timestamp: SystemTime,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
}

mod payload_serde {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S>(payload: &Vec<u8>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        STANDARD.encode(payload).serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s: String = String::deserialize(deserializer)?;
        STANDARD.decode(&s).map_err(serde::de::Error::custom)
    }
}

impl EventRecord {
    pub fn new(event_name: impl Into<String>, payload: Vec<u8>, sequence: u64) -> Self {
        EventRecord {
            event_name: event_name.into(),
            payload,
            sequence,
            timestamp: SystemTime::now(),
            metadata: HashMap::new(),
        }
    }

    /// Create an event record with metadata.
    pub fn with_metadata(
        event_name: impl Into<String>,
        payload: Vec<u8>,
        sequence: u64,
        metadata: HashMap<String, String>,
    ) -> Self {
        EventRecord {
            event_name: event_name.into(),
            payload,
            sequence,
            timestamp: SystemTime::now(),
            metadata,
        }
    }

    /// Deserialize the payload into the specified type.
    pub fn decode<T: DeserializeOwned>(&self) -> Result<T, PayloadError> {
        bitcode::deserialize(&self.payload).map_err(|e| PayloadError {
            message: e.to_string(),
        })
    }

    /// Get the raw payload bytes.
    pub fn payload_bytes(&self) -> &[u8] {
        &self.payload
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new() {
        let payload = bitcode::serialize(&("arg1", "arg2")).unwrap();
        let event_record = EventRecord::new("test_event", payload.clone(), 1);
        assert_eq!(event_record.event_name, "test_event");
        assert_eq!(event_record.payload, payload);
        assert_eq!(event_record.sequence, 1);
    }

    #[test]
    fn clone() {
        let payload = bitcode::serialize(&("arg1", "arg2")).unwrap();
        let original = EventRecord::new("test_event", payload, 1);
        let cloned = original.clone();
        assert_eq!(cloned.event_name, "test_event");
        assert_eq!(cloned.payload, original.payload);
        assert_eq!(cloned.sequence, 1);
    }

    #[test]
    fn debug() {
        let payload = bitcode::serialize(&("arg1", "arg2")).unwrap();
        let event_record = EventRecord::new("test_event", payload, 1);
        let debug_str = format!("{:?}", event_record);
        assert!(debug_str.contains("EventRecord"));
        assert!(debug_str.contains("event_name: \"test_event\""));
        assert!(debug_str.contains("sequence: 1"));
    }

    #[test]
    fn serialize_deserialize() {
        let payload = bitcode::serialize(&("arg1", "arg2")).unwrap();
        let event_record = EventRecord::new("test_event", payload.clone(), 1);
        let serialized = serde_json::to_string(&event_record).unwrap();
        let deserialized: EventRecord = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.event_name, "test_event");
        assert_eq!(deserialized.payload, payload);
        assert_eq!(deserialized.sequence, 1);
        assert_eq!(deserialized.timestamp, event_record.timestamp);
    }

    #[test]
    fn decode_payload() {
        let payload = bitcode::serialize(&("hello", 42i32, true)).unwrap();
        let event_record = EventRecord::new("test_event", payload, 1);
        let decoded: (String, i32, bool) = event_record.decode().unwrap();
        assert_eq!(decoded, ("hello".to_string(), 42, true));
    }

    #[test]
    fn payload_bytes() {
        let payload = vec![0xff, 0x00, 0xab];
        let event_record = EventRecord::new("test_event", payload.clone(), 1);
        assert_eq!(event_record.payload_bytes(), &payload[..]);
    }

    #[test]
    fn with_metadata_constructor() {
        let mut meta = HashMap::new();
        meta.insert("correlation_id".to_string(), "req-123".to_string());
        meta.insert("user_id".to_string(), "u-1".to_string());

        let record = EventRecord::with_metadata("test_event", vec![], 1, meta);
        assert_eq!(record.correlation_id(), Some("req-123"));
        assert_eq!(record.meta("user_id"), Some("u-1"));
        assert_eq!(record.causation_id(), None);
    }

    #[test]
    fn metadata_skipped_when_empty_in_serialization() {
        let record = EventRecord::new("test_event", vec![], 1);
        let json = serde_json::to_string(&record).unwrap();
        assert!(!json.contains("metadata"));

        let mut meta = HashMap::new();
        meta.insert("key".to_string(), "val".to_string());
        let record_with_meta = EventRecord::with_metadata("test_event", vec![], 1, meta);
        let json = serde_json::to_string(&record_with_meta).unwrap();
        assert!(json.contains("metadata"));
        assert!(json.contains("key"));
    }

    #[test]
    fn deserialize_without_metadata_field() {
        // Simulates loading old events that were serialized before metadata existed
        let json = r#"{"event_name":"old_event","payload":"","sequence":1,"timestamp":{"secs_since_epoch":0,"nanos_since_epoch":0}}"#;
        let record: EventRecord = serde_json::from_str(json).unwrap();
        assert!(record.metadata.is_empty());
        assert_eq!(record.correlation_id(), None);
    }
}
