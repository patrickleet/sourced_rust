use serde::{Deserialize, Serialize};
use serde_json;
use sourced_rust::{Entity, EventRecord};

#[derive(Serialize, Deserialize, Debug, Default)]
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
            "Initialize",
            vec![id, self.user_id.clone(), self.task.clone()],
        );
        self.entity
            .enqueue("ToDoInitialized", serde_json::to_string(self).unwrap());
    }

    pub fn complete(&mut self) {
        if !self.completed {
            self.completed = true;
            self.entity
                .digest("Complete", vec![self.entity.id().to_string()]);
            self.entity
                .enqueue("ToDoCompleted", serde_json::to_string(self).unwrap());
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

    pub fn deserialize(data: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(data)
    }
}

enum TodoEvent {
    Initialize { id: String, user_id: String, task: String },
    Complete,
}

impl TodoEvent {
    fn apply(self, todo: &mut Todo) {
        match self {
            TodoEvent::Initialize { id, user_id, task } => todo.initialize(id, user_id, task),
            TodoEvent::Complete => todo.complete(),
        }
    }
}

impl TryFrom<&EventRecord> for TodoEvent {
    type Error = String;

    fn try_from(event: &EventRecord) -> Result<Self, Self::Error> {
        match event.event_name.as_str() {
            "Initialize" => match event.args.as_slice() {
                [id, user_id, task] => Ok(TodoEvent::Initialize {
                    id: id.clone(),
                    user_id: user_id.clone(),
                    task: task.clone(),
                }),
                _ => Err("Invalid number of arguments for Initialize method".to_string()),
            },
            "Complete" => Ok(TodoEvent::Complete),
            _ => Err(format!("Unknown method: {}", event.event_name)),
        }
    }
}

sourced_rust::impl_aggregate!(Todo, entity, replay_event);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TodoSnapshot {
    pub id: String,
    pub user_id: String,
    pub task: String,
    pub completed: bool,
}
