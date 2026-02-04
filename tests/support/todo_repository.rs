use sourced_rust::{
    AggregateBuilder, AggregateRepository, HashMapRepository, Queueable, QueuedRepository,
};

use super::todo::Todo;

pub struct TodoRepository {
    inner: AggregateRepository<QueuedRepository<HashMapRepository>, Todo>,
}

impl TodoRepository {
    pub fn new() -> Self {
        TodoRepository {
            inner: HashMapRepository::new()
                .queued()
                .aggregate::<Todo>(),
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
