use sourced_rust::{digest, Entity, Snapshot};

#[derive(Default, Snapshot)]
pub struct Todo {
    pub entity: Entity,
    pub user_id: String,
    pub task: String,
    pub completed: bool,
}

impl Todo {
    pub fn new() -> Self {
        Self::default()
    }

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

sourced_rust::aggregate!(Todo, entity {
    "Initialized"(id, user_id, task) => initialize,
    "Completed"() => complete(),
});
