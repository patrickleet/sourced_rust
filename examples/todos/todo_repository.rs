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
        // Get the generic entity from the base repository
        if let Some(rehydrated_entity) = self.repository.find_by_id(id) {
            let mut todo = Todo::new();
            todo.entity = rehydrated_entity;
            
            // Use the rehydrate method we added to Todo
            if let Err(e) = todo.rehydrate() {
                eprintln!("Error rehydrating todo: {}", e);
                return None;
            }
            
            Some(todo)
        } else {
            None
        }
    }

    pub fn commit(&self, todo: &mut Todo) -> Result<(), String> {
        self.repository.commit(&mut todo.entity)
    }
}
