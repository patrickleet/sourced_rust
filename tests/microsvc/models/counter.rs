//! Test domain: a simple Counter aggregate for microsvc tests.

use serde::{Deserialize, Serialize};
use sourced_rust::{sourced, Entity, Snapshot};

/// A simple counter aggregate for testing microsvc dispatch.
#[derive(Default, Snapshot)]
pub struct Counter {
    pub entity: Entity,
    pub value: i64,
}

#[sourced(entity)]
impl Counter {
    #[event("Created")]
    pub fn create(&mut self, id: String) {
        self.entity.set_id(&id);
        self.value = 0;
    }

    #[event("Incremented")]
    pub fn increment(&mut self, amount: i64) {
        self.value += amount;
    }

    #[event("Decremented", when = self.value >= amount)]
    pub fn decrement(&mut self, amount: i64) {
        self.value -= amount;
    }
}

/// Command payload: create a new counter.
#[derive(Serialize, Deserialize)]
pub struct CreateCounter {
    pub id: String,
}

/// Command payload: increment a counter.
#[derive(Serialize, Deserialize)]
pub struct IncrementCounter {
    pub id: String,
    pub amount: i64,
}

/// Command payload: decrement a counter.
#[derive(Serialize, Deserialize)]
pub struct DecrementCounter {
    pub id: String,
    pub amount: i64,
}
