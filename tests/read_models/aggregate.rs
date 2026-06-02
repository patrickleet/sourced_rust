//! Simple Counter aggregate for testing read models.

use distributed::{sourced, Entity};

#[derive(Default)]
pub struct Counter {
    pub entity: Entity,
    name: String,
    user_id: String,
    value: i32,
}

#[sourced(entity)]
impl Counter {
    pub fn new() -> Self {
        Self::default()
    }

    #[event("initialized")]
    pub fn create(&mut self, id: String, name: String, user_id: String) {
        self.entity.set_id(&id);
        self.name = name;
        self.user_id = user_id;
        self.value = 0;
    }

    #[event("incremented")]
    pub fn increment(&mut self, amount: i32) {
        self.value += amount;
    }

    #[event("decremented")]
    pub fn decrement(&mut self, amount: i32) {
        self.value -= amount;
    }

    pub fn value(&self) -> i32 {
        self.value
    }
}
