use serde::{Deserialize, Serialize};
use sourced_rust::{digest, Entity, Snapshottable};

#[derive(Default)]
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

    pub fn snapshot(&self) -> TodoSnapshot {
        TodoSnapshot {
            id: self.entity.id().to_string(),
            user_id: self.user_id.clone(),
            task: self.task.clone(),
            completed: self.completed,
        }
    }
}

sourced_rust::aggregate!(Todo, entity {
    "Initialized"(id, user_id, task) => initialize,
    "Completed"() => complete(),
});

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TodoSnapshot {
    pub id: String,
    pub user_id: String,
    pub task: String,
    pub completed: bool,
}

impl Snapshottable for Todo {
    type Snapshot = TodoSnapshot;

    fn create_snapshot(&self) -> TodoSnapshot {
        self.snapshot()
    }

    fn restore_from_snapshot(&mut self, s: TodoSnapshot) {
        self.entity.set_id(&s.id);
        self.user_id = s.user_id;
        self.task = s.task;
        self.completed = s.completed;
    }
}
