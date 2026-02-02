use serde::{Deserialize, Serialize};
use serde_json;
use sourced_rust::{Entity, EventRecord};

#[derive(Serialize, Deserialize, Debug)]
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
        self.entity.record_event(
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
                .record_event("Complete", vec![self.entity.id().to_string()]);
            self.entity
                .enqueue("ToDoCompleted", serde_json::to_string(self).unwrap());
        }
    }

    pub fn replay_event(&mut self, event: &EventRecord) -> Result<(), String> {
        match event.event_name.as_str() {
            "Initialize" if event.args.len() == 3 => {
                self.initialize(
                    event.args[0].clone(),
                    event.args[1].clone(),
                    event.args[2].clone(),
                );
                Ok(())
            }
            "Initialize" => Err("Invalid number of arguments for Initialize method".to_string()),
            "Complete" => {
                self.complete();
                Ok(())
            }
            _ => Err(format!("Unknown method: {}", event.event_name)),
        }
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TodoSnapshot {
    pub id: String,
    pub user_id: String,
    pub task: String,
    pub completed: bool,
}
