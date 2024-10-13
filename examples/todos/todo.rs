use sourced_rust::{Entity, CommandRecord};
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
        self.user_id = user_id.clone();
        self.task = task.clone();
        self.completed = false;

        self.entity.digest("Initialize".to_string(), vec![id, user_id.clone(), task.clone()]);
        let serialized = serde_json::to_string(self).unwrap_or_else(|e| format!("{{\"error\": \"{}\"}}", e));
        self.entity.enqueue("ToDoInitialized".to_string(), serialized);
    }

    pub fn complete(&mut self) {
        if !self.completed {
            self.completed = true;

            self.entity.digest("Complete".to_string(), vec![self.entity.id.clone()]);
            let serialized = serde_json::to_string(self).unwrap_or_else(|e| format!("{{\"error\": \"{}\"}}", e));
            self.entity.enqueue("ToDoCompleted".to_string(), serialized);
        }
    }

    pub fn replay_command(&mut self, cmd: CommandRecord) -> Result<Option<String>, String> {
        match cmd.command_name.as_str() {
            "Initialize" => {
                if cmd.args.len() == 3 {
                    self.initialize(
                        cmd.args[0].clone(),
                        cmd.args[1].clone(),
                        cmd.args[2].clone(),
                    );
                    Ok(Some("ToDoInitialized".to_string()))
                } else {
                    Err("Invalid number of arguments for Initialize command".to_string())
                }
            }
            "Complete" => {
                self.complete();
                Ok(Some("ToDoCompleted".to_string()))
            }
            _ => Err(format!("Unknown command: {}", cmd.command_name)),
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

    pub fn on<F>(&mut self, event: &str, listener: F)
    where
        F: Fn(String) + Send + Sync + 'static,
    {
        self.entity.on(event, listener);
    }

    pub fn rehydrate(&mut self) -> Result<(), String> {
        self.entity.replaying = true;
        let mut events_to_emit = Vec::new();
        for command in self.entity.commands.clone() {
            if let Ok(Some(event)) = self.replay_command(command) {
                events_to_emit.push(event);
            }
        }
        self.entity.replaying = false;

        // Emit events after replaying
        for event in events_to_emit {
            let serialized = serde_json::to_string(self).unwrap_or_else(|e| format!("{{\"error\": \"{}\"}}", e));
            self.entity.enqueue(event, serialized);
        }
        self.entity.emit_queued_events();

        Ok(())
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
