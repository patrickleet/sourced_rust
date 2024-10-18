use serde::{Serialize, Deserialize};

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct EventRecord {
    pub event_name: String,
    pub args: Vec<String>,
}