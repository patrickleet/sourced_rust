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

    pub fn find_by_id(&self, id: &str) -> Option<Todo> {
        let entity = self.repository.find_by_id(id)?;
        let mut todo = Todo::new();
        todo.entity = entity;
        todo.rehydrate().ok()?; 
        Some(todo)
    }

    pub fn commit(&self, todo: &mut Todo) -> Result<(), String> {
        self.repository.commit(&mut todo.entity)
    }
}
