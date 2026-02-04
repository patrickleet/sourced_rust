use sourced_rust::{
    AggregateBuilder, AggregateRepository, HashMapRepository, OutboxRepositoryExt, Queueable,
    QueuedRepository, RepositoryError,
};
use std::ops::{Deref, DerefMut};
use std::time::Duration;

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

    pub fn outbox_messages_pending(
        &self,
    ) -> Result<Vec<sourced_rust::OutboxMessage>, RepositoryError> {
        self.inner.repo().inner().outbox_messages_pending()
    }

    pub fn claim_outbox_messages(
        &self,
        worker_id: &str,
        batch_size: usize,
        lease: Duration,
    ) -> Result<Vec<sourced_rust::OutboxMessage>, RepositoryError> {
        self.inner
            .repo()
            .inner()
            .claim_outbox_messages(worker_id, batch_size, lease)
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
