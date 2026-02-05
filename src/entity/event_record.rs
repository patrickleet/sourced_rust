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
}
