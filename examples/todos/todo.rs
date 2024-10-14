use sourced_rust::{Entity, EventRecord};
use serde::{Serialize, Deserialize};
use serde_json;

#[derive(Serialize, Deserialize, Debug)]
pub struct Todo {
    pub entity: Entity,
    user_id: String,
    task: String,
    completed: bool,
}

impl Clone for Todo {
    fn clone(&self) -> Self {
        Todo {
            entity: self.entity.clone(),
            user_id: self.user_id.clone(),
            task: self.task.clone(),
            completed: self.completed,
        }
    }
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
        self.entity.id = id.clone();
        self.user_id = user_id;
        self.task = task;
        self.completed = false;

        self.entity.digest("Initialize".to_string(), vec![id, self.user_id.clone(), self.task.clone()]);
        self.entity.enqueue("ToDoInitialized".to_string(), serde_json::to_string(self).unwrap());
    }

    pub fn complete(&mut self) {
        if !self.completed {
            self.completed = true;
            self.entity.digest("Complete".to_string(), vec![self.entity.id.clone()]);
            self.entity.enqueue("ToDoCompleted".to_string(), serde_json::to_string(self).unwrap());
        }
    }

    pub fn replay_event(&mut self, event: EventRecord) -> Result<Option<String>, String> {
        match event.event_name.as_str() {
            "Initialize" if event.args.len() == 3 => {
                self.initialize(
                    event.args[0].clone(), 
                    event.args[1].clone(), 
                    event.args[2].clone());
                Ok(Some("ToDoInitialized".to_string()))
            }
            "Initialize" => Err("Invalid number of arguments for Initialize method".to_string()),
            "Complete" => {
                self.complete();
                Ok(Some("ToDoCompleted".to_string()))
            }
            _ => Err(format!("Unknown method: {}", event.event_name)),
        }
    }

    pub fn snapshot(&self) -> TodoSnapshot {
        TodoSnapshot {
            id: self.entity.id.clone(),
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
