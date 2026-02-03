use sourced_rust::{AggregateRepository, HashMapRepository, Outboxable, Queueable, QueuedRepository};

use super::todo::Todo;

pub struct TodoRepository {
    inner: AggregateRepository<QueuedRepository<HashMapRepository>, Todo>,
}

impl TodoRepository {
    pub fn new() -> Self {
        TodoRepository {
            inner: AggregateRepository::new(HashMapRepository::new().queued().with_outbox()),
        }
    }
}

impl std::ops::Deref for TodoRepository {
    type Target = AggregateRepository<QueuedRepository<HashMapRepository>, Todo>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl std::ops::DerefMut for TodoRepository {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}
