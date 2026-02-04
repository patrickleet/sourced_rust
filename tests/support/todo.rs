use serde::{Deserialize, Serialize};
use sourced_rust::{digest, Entity};

#[derive(Default)]
pub struct Todo {
    pub entity: Entity,
    user_id: String,
    task: String,
    completed: bool,
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

    #[digest("Completed", id, when = !self.completed)]
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
    "Completed"(id) => complete(),
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TodoSnapshot {
    pub id: String,
    pub user_id: String,
    pub task: String,
    pub completed: bool,
}
