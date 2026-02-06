//! Read model views for Counter.

use serde::{Deserialize, Serialize};
use sourced_rust::ReadModel;

/// A read-optimized view of a single counter.
#[derive(Clone, Debug, Serialize, Deserialize, ReadModel)]
#[readmodel(collection = "counter_views")]
pub struct CounterView {
    #[readmodel(id)]
    pub id: String,
    pub name: String,
    pub user_id: String,
    pub value: i32,
}

impl CounterView {
    pub fn new(id: &str, name: &str, user_id: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            user_id: user_id.to_string(),
            value: 0,
        }
    }

    pub fn set_value(&mut self, value: i32) {
        self.value = value;
    }
}

/// Index of all counters belonging to a user.
#[derive(Clone, Debug, Serialize, Deserialize, ReadModel)]
#[readmodel(collection = "user_counters")]
pub struct UserCountersIndex {
    #[readmodel(id)]
    pub user_id: String,
    pub counter_ids: Vec<String>,
    pub total_value: i32,
}

impl UserCountersIndex {
    pub fn new(user_id: &str) -> Self {
        Self {
            user_id: user_id.to_string(),
            counter_ids: vec![],
            total_value: 0,
        }
    }

    pub fn add_counter(&mut self, counter_id: &str, value: i32) {
        if !self.counter_ids.contains(&counter_id.to_string()) {
            self.counter_ids.push(counter_id.to_string());
        }
        self.total_value += value;
    }
}
