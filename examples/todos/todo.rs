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
        self.entity.id = id.clone();
        self.user_id = user_id;
        self.task = task;
        self.completed = false;

        self.entity.digest(
            "Initialize".to_string(),
            vec![id, self.user_id.clone(), self.task.clone()],
        );
        self.entity.enqueue(
            "ToDoInitialized".to_string(),
            serde_json::to_string(self).unwrap(),
        );
    }

    pub fn complete(&mut self) {
        if !self.completed {
            self.completed = true;
            self.entity
                .digest("Complete".to_string(), vec![self.entity.id.clone()]);
            self.entity.enqueue(
                "ToDoCompleted".to_string(),
                serde_json::to_string(self).unwrap(),
            );
        }
    }

    pub fn replay_event(&mut self, event: EventRecord) -> Result<Option<String>, String> {
        match event.event_name.as_str() {
            "Initialize" if event.args.len() == 3 => {
                self.initialize(
                    event.args[0].clone(),
                    event.args[1].clone(),
                    event.args[2].clone(),
                );
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

#[cfg(test)]
mod tests {
    use super::super::todo::Todo;
    use super::super::todo_repository::TodoRepository;

    #[test]
    fn test_workflow() {
        let repo = TodoRepository::new();

        // Create a new Todo
        let mut todo = Todo::new();
        todo.initialize(
            "1".to_string(),
            "user1".to_string(),
            "Buy groceries".to_string(),
        );

        // Add event listeners
        todo.entity
            .on("ToDoInitialized", |data| match Todo::deserialize(&data) {
                Ok(deserialized_todo) => {
                    println!("Todo Initialized: {:?}", deserialized_todo.snapshot());
                    assert!(deserialized_todo.snapshot().id == "1");
                    assert!(deserialized_todo.snapshot().user_id == "user1");
                    assert!(deserialized_todo.snapshot().task == "Buy groceries");
                    assert!(!deserialized_todo.snapshot().completed);
                }
                Err(e) => {
                    println!("Error deserializing Todo: {}", e);
                }
            });

        // Commit the Todo to the repository
        let _ = repo.commit(&mut todo);

        // Retrieve the Todo from the repository
        if let Some(mut retrieved_todo) = repo.get("1") {
            println!("Retrieved Todo: {:?}", retrieved_todo);

            retrieved_todo
                .entity
                .on("ToDoCompleted", |data| match Todo::deserialize(&data) {
                    Ok(deserialized_todo) => {
                        println!("Todo Completed: {:?}", deserialized_todo.snapshot());
                        assert!(deserialized_todo.snapshot().id == "1");
                        assert!(deserialized_todo.snapshot().user_id == "user1");
                        assert!(deserialized_todo.snapshot().task == "Buy groceries");
                        assert!(deserialized_todo.snapshot().completed);
                    }
                    Err(e) => {
                        println!("Error deserializing Todo: {}", e);
                    }
                });

            // Complete the Todo
            retrieved_todo.complete();

            // Commit the changes
            let _ = repo.commit(&mut retrieved_todo);

            // Retrieve the Todo again to demonstrate that events are fired on retrieval
            if let Some(updated_todo) = repo.get("1") {
                println!("Updated Todo: {:?}", updated_todo.snapshot());
                assert!(updated_todo.snapshot().id == "1");
                assert!(updated_todo.snapshot().user_id == "user1");
                assert!(updated_todo.snapshot().task == "Buy groceries");
                assert!(updated_todo.snapshot().completed);
            } else {
                println!("Updated Todo not found");
            }
        } else {
            println!("Todo not found");
        }

        let mut todo2 = Todo::new();
        todo2.initialize(
            "2".to_string(),
            "user1".to_string(),
            "Buy Sauna".to_string(),
        );

        let mut todo3 = Todo::new();
        todo3.initialize(
            "3".to_string(),
            "user2".to_string(),
            "Chew bubblegum".to_string(),
        );

        // Commit multiple Todos to the repository
        let _ = repo.commit_all(&mut [&mut todo2, &mut todo3]);

        // get all the todos from the repository
        let all_todos = repo.get_all(&["1", "2", "3"]);
        if all_todos.len() > 0 {
            println!("All Todos: {:?}", all_todos);
            assert!(all_todos.len() == 3);
        } else {
            println!("No Todos found");
        }
    }
}
