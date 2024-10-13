use sourced_rust::{Entity, CommandRecord};
use std::any::Any;

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
        self.entity.id = id.clone();
        self.user_id = user_id.clone();
        self.task = task.clone();
        self.completed = false;

        self.entity.digest("Initialize".to_string(), vec![id, user_id, task]);
        self.entity.enqueue("ToDoInitialized".to_string());
    }

    pub fn complete(&mut self) {
        if !self.completed {
            self.completed = true;

            self.entity.digest("Complete".to_string(), vec![self.entity.id.clone()]);
            self.entity.enqueue("ToDoCompleted".to_string());
        }
    }

    pub fn replay_command(&mut self, cmd: CommandRecord) -> Result<(), String> {
        match cmd.command_name.as_str() {
            "Initialize" => {
                if cmd.args.len() == 3 {
                    self.initialize(
                        cmd.args[0].clone(),
                        cmd.args[1].clone(),
                        cmd.args[2].clone(),
                    );
                    Ok(())
                } else {
                    Err("Invalid number of arguments for Initialize command".to_string())
                }
            }
            "Complete" => {
                self.complete();
                Ok(())
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

    pub fn on<F>(&self, event: String, listener: F)
    where
        F: Fn(&dyn Any) + Send + Sync + 'static,
    {
        self.entity.on(event, listener);
    }

    pub fn rehydrate(&mut self) -> Result<(), String> {
        self.entity.replaying = true;
        for command in self.entity.commands.clone() {
            self.replay_command(command)?;
        }
        self.entity.replaying = false;
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct TodoSnapshot {
    pub id: String,
    pub user_id: String,
    pub task: String,
    pub completed: bool,
}
