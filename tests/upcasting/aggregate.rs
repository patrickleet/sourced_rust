use serde::{Deserialize, Serialize};
use sourced_rust::{digest, Entity, Snapshottable};

// =============================================================================
// V1 aggregate: original schema
// =============================================================================

/// A simple Todo at v1 — no priority field.
#[derive(Default)]
pub struct TodoV1 {
    pub entity: Entity,
    pub user_id: String,
    pub task: String,
    pub completed: bool,
}

impl TodoV1 {
    #[digest("Initialized")]
    pub fn initialize(&mut self, id: String, user_id: String, task: String) {
        self.entity.set_id(&id);
        self.user_id = user_id;
        self.task = task;
    }

    #[digest("Completed", when = !self.completed)]
    pub fn complete(&mut self) {
        self.completed = true;
    }
}

sourced_rust::aggregate!(TodoV1, entity {
    "Initialized"(id, user_id, task) => initialize,
    "Completed"() => complete(),
});

// =============================================================================
// V2 aggregate: added priority field + upcaster
// =============================================================================

/// Upcasts Initialized v1 (id, user_id, task) → v2 (id, user_id, task, priority)
pub fn upcast_initialized_v1_v2(payload: &[u8]) -> Vec<u8> {
    let (id, user_id, task): (String, String, String) =
        bitcode::deserialize(payload).unwrap();
    bitcode::serialize(&(id, user_id, task, 0u8)).unwrap()
}

#[derive(Default)]
pub struct TodoV2 {
    pub entity: Entity,
    pub user_id: String,
    pub task: String,
    pub priority: u8,
    pub completed: bool,
}

impl TodoV2 {
    #[digest("Initialized", version = 2)]
    pub fn initialize(&mut self, id: String, user_id: String, task: String, priority: u8) {
        self.entity.set_id(&id);
        self.user_id = user_id;
        self.task = task;
        self.priority = priority;
    }

    #[digest("Completed", when = !self.completed)]
    pub fn complete(&mut self) {
        self.completed = true;
    }
}

sourced_rust::aggregate!(TodoV2, entity {
    "Initialized"(id, user_id, task, priority) => initialize,
    "Completed"() => complete(),
} upcasters [
    ("Initialized", 1 => 2, upcast_initialized_v1_v2),
]);

// =============================================================================
// V3 aggregate: added due_date field + chained upcasters
// =============================================================================

/// Upcasts Initialized v2 (id, user_id, task, priority) → v3 (id, user_id, task, priority, due_date)
pub fn upcast_initialized_v2_v3(payload: &[u8]) -> Vec<u8> {
    let (id, user_id, task, priority): (String, String, String, u8) =
        bitcode::deserialize(payload).unwrap();
    // Default due_date: empty string meaning "no due date"
    bitcode::serialize(&(id, user_id, task, priority, String::new())).unwrap()
}

#[derive(Default)]
pub struct TodoV3 {
    pub entity: Entity,
    pub user_id: String,
    pub task: String,
    pub priority: u8,
    pub due_date: String,
    pub completed: bool,
}

impl TodoV3 {
    #[digest("Initialized", version = 3)]
    pub fn initialize(&mut self, id: String, user_id: String, task: String, priority: u8, due_date: String) {
        self.entity.set_id(&id);
        self.user_id = user_id;
        self.task = task;
        self.priority = priority;
        self.due_date = due_date;
    }

    #[digest("Completed", when = !self.completed)]
    pub fn complete(&mut self) {
        self.completed = true;
    }
}

sourced_rust::aggregate!(TodoV3, entity {
    "Initialized"(id, user_id, task, priority, due_date) => initialize,
    "Completed"() => complete(),
} upcasters [
    ("Initialized", 1 => 2, upcast_initialized_v1_v2),
    ("Initialized", 2 => 3, upcast_initialized_v2_v3),
]);

// =============================================================================
// Snapshottable impl for TodoV2 (needed for snapshot + upcasting tests)
// =============================================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TodoV2Snapshot {
    pub id: String,
    pub user_id: String,
    pub task: String,
    pub priority: u8,
    pub completed: bool,
}

impl Snapshottable for TodoV2 {
    type Snapshot = TodoV2Snapshot;

    fn create_snapshot(&self) -> TodoV2Snapshot {
        TodoV2Snapshot {
            id: self.entity.id().to_string(),
            user_id: self.user_id.clone(),
            task: self.task.clone(),
            priority: self.priority,
            completed: self.completed,
        }
    }

    fn restore_from_snapshot(&mut self, s: TodoV2Snapshot) {
        self.entity.set_id(&s.id);
        self.user_id = s.user_id;
        self.task = s.task;
        self.priority = s.priority;
        self.completed = s.completed;
    }
}
