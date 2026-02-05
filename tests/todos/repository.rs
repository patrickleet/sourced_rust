use sourced_rust::{AggregateBuilder, AggregateRepository, HashMapRepository, Queueable, QueuedRepository};
use std::ops::{Deref, DerefMut};

use super::aggregate::Todo;

type Inner = AggregateRepository<QueuedRepository<HashMapRepository>, Todo>;

pub struct TodoRepository {
    inner: Inner,
}

impl TodoRepository {
    pub fn new() -> Self {
        Self {
            inner: HashMapRepository::new().queued().aggregate::<Todo>(),
        }
    }

    /// Access the underlying repository for projection and commit operations.
    pub fn repo(&self) -> &QueuedRepository<HashMapRepository> {
        self.inner.repo()
    }
}

impl Deref for TodoRepository {
    type Target = Inner;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for TodoRepository {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}
