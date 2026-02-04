use std::fmt;
use std::time::SystemTime;
use serde::{Serialize, Deserialize};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArgCountError {
    pub expected: usize,
    pub actual: usize,
}

impl fmt::Display for ArgCountError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "expected {} args, got {}",
            self.expected, self.actual
        )
    }
}

impl std::error::Error for ArgCountError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArgParseError {
    pub index: usize,
    pub value: String,
    pub message: String,
}

impl fmt::Display for ArgParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "failed to parse arg {} ('{}'): {}",
            self.index, self.value, self.message
        )
    }
}

impl std::error::Error for ArgParseError {}

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

    pub fn arg(&self, index: usize) -> Option<&str> {
        self.args.get(index).map(String::as_str)
    }

    pub fn arg_parse<T>(&self, index: usize) -> Result<T, ArgParseError>
    where
        T: std::str::FromStr,
        T::Err: fmt::Display,
    {
        let value = self
            .arg(index)
            .ok_or_else(|| ArgParseError {
                index,
                value: String::new(),
                message: "missing argument".to_string(),
            })?;
        value.parse::<T>().map_err(|err| ArgParseError {
            index,
            value: value.to_string(),
            message: err.to_string(),
        })
    }

    pub fn args2(&self) -> Result<(&str, &str), ArgCountError> {
        match self.args.as_slice() {
            [a, b] => Ok((a.as_str(), b.as_str())),
            _ => Err(ArgCountError {
                expected: 2,
                actual: self.args.len(),
            }),
        }
    }

    pub fn args3(&self) -> Result<(&str, &str, &str), ArgCountError> {
        match self.args.as_slice() {
            [a, b, c] => Ok((a.as_str(), b.as_str(), c.as_str())),
            _ => Err(ArgCountError {
                expected: 3,
                actual: self.args.len(),
            }),
        }
    }
}

#[macro_export]
macro_rules! event_map {
    ($event_ty:ty, { $($name:literal => ($($arg:ident),*) => $variant:ident),+ $(,)? }) => {
        impl ::std::convert::TryFrom<&$crate::EventRecord> for $event_ty {
            type Error = String;

            fn try_from(event: &$crate::EventRecord) -> Result<Self, Self::Error> {
                match event.event_name.as_str() {
                    $(
                        $name => {
                            $crate::event_map!(@arm event, $variant, ($($arg),*))
                        }
                    ),+
                    ,
                    _ => Err(format!("Unknown method: {}", event.event_name)),
                }
            }
        }
    };

    (@arm $event:ident, $variant:ident, ()) => {{
        if !$event.args.is_empty() {
            return Err(format!(
                "Invalid number of arguments for {} method",
                $event.event_name
            ));
        }
        Ok(Self::$variant)
    }};

    (@arm $event:ident, $variant:ident, ($($arg:ident),+)) => {{
        let expected = $crate::event_map!(@count $($arg),+);
        let actual = $event.args.len();
        if actual != expected {
            return Err(format!(
                "Invalid number of arguments for {} method",
                $event.event_name
            ));
        }
        let mut iter = $event.args.iter();
        $(
            let $arg = iter.next().unwrap().clone();
        )+
        Ok(Self::$variant { $($arg),+ })
    }};

    (@count $($arg:ident),+) => {
        <[()]>::len(&[$($crate::event_map!(@unit $arg)),+])
    };

    (@unit $arg:ident) => { () };
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

    #[test]
    fn arg_helpers() {
        let event_record = EventRecord::new(
            "test_event",
            vec!["1".to_string(), "two".to_string(), "3".to_string()],
            1,
        );
        assert_eq!(event_record.arg(0), Some("1"));
        assert_eq!(event_record.arg(3), None);
        assert_eq!(event_record.args3().unwrap(), ("1", "two", "3"));
        assert!(event_record.args2().is_err());
        let parsed: i32 = event_record.arg_parse(0).unwrap();
        assert_eq!(parsed, 1);
        assert!(event_record.arg_parse::<i32>(1).is_err());
    }
}
