use serde::{Deserialize, Serialize};
use sourced_rust::{Entity, EventRecord};

pub struct Todo {
    pub entity: Entity,
    user_id: String,
    task: String,
    completed: bool,
}

impl Todo {
    pub fn new() -> Self {
        Todo {
            entity: Entity::new(),
            user_id: String::new(),
            task: String::new(),
            completed: false,
        }
    }

    pub fn initialize(&mut self, id: String, user_id: String, task: String) {
        self.entity.set_id(&id);
        self.user_id = user_id;
        self.task = task;
        self.completed = false;
        self.entity.digest(
            "Initialized",
            vec![id, self.user_id.clone(), self.task.clone()],
        );
    }

    pub fn complete(&mut self) {
        if !self.completed {
            self.completed = true;
            let id = self.entity.id().to_string();
            self.entity.digest("Complete", vec![id]);
        }
    }

    pub fn replay_event(&mut self, event: &EventRecord) -> Result<(), String> {
        TodoEvent::try_from(event)?.apply(self);
        Ok(())
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

impl Default for Todo {
    fn default() -> Self {
        Self::new()
    }
}

enum TodoEvent {
    Initialized {
        id: String,
        user_id: String,
        task: String,
    },
    Completed {
        id: String,
    },
}

impl TodoEvent {
    fn apply(self, todo: &mut Todo) {
        match self {
            TodoEvent::Initialized { id, user_id, task } => todo.initialize(id, user_id, task),
            TodoEvent::Completed { id } => {
                let _ = id;
                todo.complete();
            }
        }
    }
}

sourced_rust::aggregate!(Todo, entity, replay_event, TodoEvent, {
    "Initialized" => (id, user_id, task) => Initialized,
    "Complete" => (id) => Completed,
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TodoSnapshot {
    pub id: String,
    pub user_id: String,
    pub task: String,
    pub completed: bool,
}
