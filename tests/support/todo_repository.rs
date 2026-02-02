use sourced_rust::{
    Entity, HashMapRepository, Queueable, QueuedRepository, Repository, RepositoryError,
};

use super::todo::Todo;

#[derive(Debug)]
pub enum TodoRepositoryError {
    Repository(RepositoryError),
    Replay(String),
}

impl std::fmt::Display for TodoRepositoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TodoRepositoryError::Repository(err) => write!(f, "repository error: {}", err),
            TodoRepositoryError::Replay(err) => write!(f, "replay error: {}", err),
        }
    }
}

impl std::error::Error for TodoRepositoryError {}

impl From<RepositoryError> for TodoRepositoryError {
    fn from(value: RepositoryError) -> Self {
        TodoRepositoryError::Repository(value)
    }
}

pub struct TodoRepository {
    repository: QueuedRepository<HashMapRepository>,
}

impl TodoRepository {
    pub fn new() -> Self {
        TodoRepository {
            repository: HashMapRepository::new().queued(),
        }
    }

    pub fn get(&self, id: &str) -> Result<Option<Todo>, TodoRepositoryError> {
        let entity = self.repository.get(id)?;
        let Some(entity) = entity else {
            return Ok(None);
        };

        let mut todo = Todo::new();
        todo.entity = entity;
        self.replay_events(&mut todo)?;

        Ok(Some(todo))
    }

    pub fn get_all(&self, ids: &[&str]) -> Result<Vec<Todo>, TodoRepositoryError> {
        let entities = self.repository.get_all(ids)?;
        let mut todos = Vec::with_capacity(entities.len());

        for entity in entities {
            let mut todo = Todo::new();
            todo.entity = entity;
            self.replay_events(&mut todo)?;
            todos.push(todo);
        }

        Ok(todos)
    }

    pub fn peek(&self, id: &str) -> Result<Option<Todo>, TodoRepositoryError> {
        let entity = self.repository.peek(id)?;
        let Some(entity) = entity else {
            return Ok(None);
        };

        let mut todo = Todo::new();
        todo.entity = entity;
        self.replay_events(&mut todo)?;

        Ok(Some(todo))
    }

    pub fn peek_all(&self, ids: &[&str]) -> Result<Vec<Todo>, TodoRepositoryError> {
        let entities = self.repository.peek_all(ids)?;
        let mut todos = Vec::with_capacity(entities.len());

        for entity in entities {
            let mut todo = Todo::new();
            todo.entity = entity;
            self.replay_events(&mut todo)?;
            todos.push(todo);
        }

        Ok(todos)
    }

    pub fn commit(&self, todo: &mut Todo) -> Result<(), TodoRepositoryError> {
        self.repository.commit(&mut todo.entity)?;
        Ok(())
    }

    pub fn commit_all(&self, todos: &mut [&mut Todo]) -> Result<(), TodoRepositoryError> {
        let mut entities: Vec<&mut Entity> =
            todos.iter_mut().map(|todo| &mut todo.entity).collect();
        self.repository.commit_all(&mut entities)?;
        Ok(())
    }

    pub fn abort(&self, todo: &Todo) -> Result<(), TodoRepositoryError> {
        self.repository.unlock(todo.entity.id())?;
        Ok(())
    }

    fn replay_events(&self, todo: &mut Todo) -> Result<(), TodoRepositoryError> {
        let events = todo.entity.events().to_vec();
        todo.entity.set_replaying(true);
        for event in &events {
            todo.replay_event(event)
                .map_err(TodoRepositoryError::Replay)?;
        }
        todo.entity.set_replaying(false);
        Ok(())
    }
}
