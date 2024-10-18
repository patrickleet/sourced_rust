use sourced_rust::{Entity, Repository, HashMapRepository};
use super::todo::Todo;
use rayon::prelude::*; 

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
