//! Projection views for Counter.

use serde::{Deserialize, Serialize};
use sourced_rust::ProjectionSchema;

/// A read-optimized view of a single counter.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CounterView {
    pub id: String,
    pub name: String,
    pub user_id: String,
    pub value: i32,
}

impl ProjectionSchema for CounterView {
    const PREFIX: &'static str = "counter_view";

    fn id(&self) -> &str {
        &self.id
    }
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
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserCountersIndex {
    pub user_id: String,
    pub counter_ids: Vec<String>,
    pub total_value: i32,
}

impl ProjectionSchema for UserCountersIndex {
    const PREFIX: &'static str = "user_counters";

    fn id(&self) -> &str {
        &self.user_id
    }
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
