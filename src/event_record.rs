use serde::{Serialize, Deserialize};

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct EventRecord {
    pub event_name: String,
    pub args: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new() {
        let event_record = EventRecord {
            event_name: String::from("test_event"),
            args: vec![String::from("arg1"), String::from("arg2")],
        };
        assert_eq!(event_record.event_name, "test_event");
        assert_eq!(event_record.args, vec!["arg1", "arg2"]);
    }

    #[test]
    fn clone() {
        let original = EventRecord {
            event_name: String::from("test_event"),
            args: vec![String::from("arg1"), String::from("arg2")],
        };
        let cloned = original.clone();
        assert_eq!(cloned.event_name, "test_event");
        assert_eq!(cloned.args, vec!["arg1", "arg2"]);
    }

    #[test]
    fn debug() {
        let event_record = EventRecord {
            event_name: String::from("test_event"),
            args: vec![String::from("arg1"), String::from("arg2")],
        };
        let debug_str = format!("{:?}", event_record);
        assert!(debug_str.contains("EventRecord"));
        assert!(debug_str.contains("event_name: \"test_event\""));
        assert!(debug_str.contains("args: [\"arg1\", \"arg2\"]"));
    }

    #[test]
    fn serialize_deserialize() {
        let event_record = EventRecord {
            event_name: String::from("test_event"),
            args: vec![String::from("arg1"), String::from("arg2")],
        };
        let serialized = serde_json::to_string(&event_record).unwrap();
        let deserialized: EventRecord = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.event_name, "test_event");
        assert_eq!(deserialized.args, vec!["arg1", "arg2"]);
    }
}
