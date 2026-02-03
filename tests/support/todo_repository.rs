use sourced_rust::{
    Entity, HashMapRepository, Outboxable, Queueable, QueuedRepository, Repository, RepositoryError,
};

use super::todo::Todo;

pub struct TodoRepository {
    repository: QueuedRepository<HashMapRepository>,
}

impl TodoRepository {
    pub fn new() -> Self {
        TodoRepository {
            repository: HashMapRepository::new().queued().with_outbox(),
        }
    }

    pub fn get(&self, id: &str) -> Result<Option<Todo>, RepositoryError> {
        let entity = self.repository.get(id)?;
        let Some(entity) = entity else {
            return Ok(None);
        };

        let mut todo = Todo::new();
        todo.entity = entity;
        self.replay_events(&mut todo)?;

        Ok(Some(todo))
    }

    pub fn get_all(&self, ids: &[&str]) -> Result<Vec<Todo>, RepositoryError> {
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

    pub fn peek(&self, id: &str) -> Result<Option<Todo>, RepositoryError> {
        let entity = self.repository.peek(id)?;
        let Some(entity) = entity else {
            return Ok(None);
        };

        let mut todo = Todo::new();
        todo.entity = entity;
        self.replay_events(&mut todo)?;

        Ok(Some(todo))
    }

    pub fn peek_all(&self, ids: &[&str]) -> Result<Vec<Todo>, RepositoryError> {
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

    pub fn commit(&self, todo: &mut Todo) -> Result<(), RepositoryError> {
        self.repository.commit(&mut todo.entity)?;
        Ok(())
    }

    pub fn commit_all(&self, todos: &mut [&mut Todo]) -> Result<(), RepositoryError> {
        let mut entities: Vec<&mut Entity> =
            todos.iter_mut().map(|todo| &mut todo.entity).collect();
        self.repository.commit_all(&mut entities)?;
        Ok(())
    }

    pub fn abort(&self, todo: &Todo) -> Result<(), RepositoryError> {
        self.repository.unlock(todo.entity.id())?;
        Ok(())
    }

    fn replay_events(&self, todo: &mut Todo) -> Result<(), RepositoryError> {
        let events = todo.entity.events().to_vec();
        todo.entity.set_replaying(true);
        for event in &events {
            todo.replay_event(event)
                .map_err(RepositoryError::Replay)?;
        }
        todo.entity.set_replaying(false);
        Ok(())
    }
}
