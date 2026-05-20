use sourced_rust::{sourced, Entity};

// =============================================================================
// V1 aggregate: original schema
// =============================================================================

#[derive(Default)]
pub struct TodoV1 {
    pub entity: Entity,
    pub user_id: String,
    pub task: String,
    pub completed: bool,
}

#[sourced(entity)]
impl TodoV1 {
    #[event("Initialized")]
    pub fn initialize(&mut self, id: String, user_id: String, task: String) {
        self.entity.set_id(&id);
        self.user_id = user_id;
        self.task = task;
    }

    #[event("Completed", when = !self.completed)]
    pub fn complete(&mut self) {
        self.completed = true;
    }
}

// =============================================================================
// V2 aggregate: added priority field + upcaster
// =============================================================================

pub fn upcast_initialized_v1_v2(payload: &[u8]) -> Vec<u8> {
    let (id, user_id, task): (String, String, String) = bitcode::deserialize(payload).unwrap();
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

#[sourced(entity, upcasters(
    ("Initialized", 1 => 2, upcast_initialized_v1_v2),
))]
impl TodoV2 {
    #[event("Initialized", version = 2)]
    pub fn initialize(&mut self, id: String, user_id: String, task: String, priority: u8) {
        self.entity.set_id(&id);
        self.user_id = user_id;
        self.task = task;
        self.priority = priority;
    }

    #[event("Completed", when = !self.completed)]
    pub fn complete(&mut self) {
        self.completed = true;
    }
}

// =============================================================================
// V3 aggregate: added due_date field + chained upcasters
// =============================================================================

pub fn upcast_initialized_v2_v3(payload: &[u8]) -> Vec<u8> {
    let (id, user_id, task, priority): (String, String, String, u8) =
        bitcode::deserialize(payload).unwrap();
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

#[sourced(entity, upcasters(
    ("Initialized", 1 => 2, upcast_initialized_v1_v2),
    ("Initialized", 2 => 3, upcast_initialized_v2_v3),
))]
impl TodoV3 {
    #[event("Initialized", version = 3)]
    pub fn initialize(
        &mut self,
        id: String,
        user_id: String,
        task: String,
        priority: u8,
        due_date: String,
    ) {
        self.entity.set_id(&id);
        self.user_id = user_id;
        self.task = task;
        self.priority = priority;
        self.due_date = due_date;
    }

    #[event("Completed", when = !self.completed)]
    pub fn complete(&mut self) {
        self.completed = true;
    }
}
