use serde::{Deserialize, Serialize};
use serde_json;
use sourced_rust::{Entity, EventRecord, Repository, HashMapRepository};
use rayon::prelude::*; 

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

pub struct TodoRepository {
    repository: HashMapRepository,
}

impl TodoRepository {
    pub fn new() -> Self {
        TodoRepository {
            repository: HashMapRepository::new(),
        }
    }

    pub fn get(&self, id: &str) -> Option<Todo> {
        let entity = self.repository.get(id)?;
        let mut todo = Todo::new();
        todo.entity = entity;

        self.replay_events(&mut todo);

        Some(todo)
    }

    pub fn get_all(&self, ids: &[&str]) -> Vec<Todo> {
        self.repository.get_all(ids)
            .into_par_iter()  // Use parallel iterator
            .map(|entity| {
                let mut todo = Todo::new();
                todo.entity = entity;

                self.replay_events(&mut todo);

                todo
            })
            .collect()
    }

    pub fn commit(&self, todo: &mut Todo) -> Result<(), String> {
        self.repository.commit(&mut todo.entity)
    }

    pub fn commit_all(&self, todos: &mut [&mut Todo]) -> Result<(), String> {
        // Extract entities from todos
        let mut entities: Vec<&mut Entity> = todos.iter_mut().map(|todo| &mut todo.entity).collect();
        
        // Commit the entities in the repository
        self.repository.commit_all(&mut entities)?;

        // Replay events for each todo after committing
        todos.par_iter_mut().for_each(|todo| {
            self.replay_events(todo);
        });

        Ok(())
    }

    fn replay_events(&self, todo: &mut Todo) {
        todo.entity.replaying = true;
        for event in todo.entity.events.clone() {
            todo.replay_event(event).ok();  // Ignore return value if no further processing is needed
        }
        todo.entity.replaying = false;
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
    use crate::Todo;
    use crate::TodoRepository;

    #[test]
    fn todos() {
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
