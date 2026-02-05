//! Simple Counter aggregate for testing projections.

use sourced_rust::{digest, Entity};

#[derive(Default)]
pub struct Counter {
    pub entity: Entity,
    name: String,
    user_id: String,
    value: i32,
}

impl Counter {
    pub fn new() -> Self {
        Self::default()
    }

    #[digest("CounterCreated")]
    pub fn create(&mut self, id: String, name: String, user_id: String) {
        self.entity.set_id(&id);
        self.name = name;
        self.user_id = user_id;
        self.value = 0;
    }

    #[digest("CounterIncremented")]
    pub fn increment(&mut self, amount: i32) {
        self.value += amount;
    }

    #[digest("CounterDecremented")]
    pub fn decrement(&mut self, amount: i32) {
        self.value -= amount;
    }

    pub fn value(&self) -> i32 {
        self.value
    }
}

sourced_rust::aggregate!(Counter, entity {
    "CounterCreated"(id, name, user_id) => create,
    "CounterIncremented"(amount) => increment,
    "CounterDecremented"(amount) => decrement,
});
