use sourced_rust::Repository;
use super::todo::Todo;

pub struct TodoRepository {
    repository: Repository,
}

impl TodoRepository {
    pub fn new() -> Self {
        TodoRepository {
            repository: Repository::new(),
        }
    }

    pub fn get(&self, id: &str) -> Option<Todo> {
        let entity = self.repository.get(id)?;
        let mut todo = Todo::new();
        todo.entity = entity;

        todo.entity.replaying = true;
        for event in todo.entity.events.clone() {
            todo.replay_event(event).ok();  // Ignore return value if no further processing is needed
        }
        todo.entity.replaying = false;

        Some(todo)
    }

    pub fn commit(&self, todo: &mut Todo) -> Result<(), String> {
        self.repository.commit(&mut todo.entity)
    }
}
