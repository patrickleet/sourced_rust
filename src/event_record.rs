use std::time::SystemTime;
use serde::{Serialize, Deserialize};

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct EventRecord {
    pub event_name: String,
    pub args: Vec<String>,
    pub sequence: u64,
    pub timestamp: SystemTime,
}

impl EventRecord {
    pub fn new(event_name: impl Into<String>, args: Vec<String>, sequence: u64) -> Self {
        EventRecord {
            event_name: event_name.into(),
            args,
            sequence,
            timestamp: SystemTime::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new() {
        let event_record = EventRecord::new(
            "test_event",
            vec![String::from("arg1"), String::from("arg2")],
            1,
        );
        assert_eq!(event_record.event_name, "test_event");
        assert_eq!(event_record.args, vec!["arg1", "arg2"]);
        assert_eq!(event_record.sequence, 1);
    }

    #[test]
    fn clone() {
        let original = EventRecord::new(
            "test_event",
            vec![String::from("arg1"), String::from("arg2")],
            1,
        );
        let cloned = original.clone();
        assert_eq!(cloned.event_name, "test_event");
        assert_eq!(cloned.args, vec!["arg1", "arg2"]);
        assert_eq!(cloned.sequence, 1);
    }

    #[test]
    fn debug() {
        let event_record = EventRecord::new(
            "test_event",
            vec![String::from("arg1"), String::from("arg2")],
            1,
        );
        let debug_str = format!("{:?}", event_record);
        assert!(debug_str.contains("EventRecord"));
        assert!(debug_str.contains("event_name: \"test_event\""));
        assert!(debug_str.contains("args: [\"arg1\", \"arg2\"]"));
        assert!(debug_str.contains("sequence: 1"));
    }

    #[test]
    fn serialize_deserialize() {
        let event_record = EventRecord::new(
            "test_event",
            vec![String::from("arg1"), String::from("arg2")],
            1,
        );
        let serialized = serde_json::to_string(&event_record).unwrap();
        let deserialized: EventRecord = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.event_name, "test_event");
        assert_eq!(deserialized.args, vec!["arg1", "arg2"]);
        assert_eq!(deserialized.sequence, 1);
        assert_eq!(deserialized.timestamp, event_record.timestamp);
    }
}
