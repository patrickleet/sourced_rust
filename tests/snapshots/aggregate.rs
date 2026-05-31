use distributed::{sourced, Entity, Snapshot};

#[derive(Default, Snapshot)]
pub struct Todo {
    pub entity: Entity,
    pub user_id: String,
    pub task: String,
    pub completed: bool,
}

#[sourced(entity)]
impl Todo {
    pub fn new() -> Self {
        Self::default()
    }

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
