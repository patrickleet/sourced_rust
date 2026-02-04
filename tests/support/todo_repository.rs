use sourced_rust::{
    AggregateBuilder, AggregateRepository, HashMapRepository, OutboxMessage, OutboxMessageStatus,
    OutboxRepositoryExt, Queueable, QueuedRepository, RepositoryError,
};
use std::time::Duration;

use super::todo::Todo;

pub struct TodoRepository {
    inner: AggregateRepository<QueuedRepository<HashMapRepository>, Todo>,
}

impl TodoRepository {
    pub fn new() -> Self {
        TodoRepository {
            inner: HashMapRepository::new().queued().aggregate::<Todo>(),
        }
    }
}

impl OutboxRepositoryExt for TodoRepository {
    fn outbox_messages_by_status(
        &self,
        status: OutboxMessageStatus,
    ) -> Result<Vec<OutboxMessage>, RepositoryError> {
        self.inner.repo().inner().outbox_messages_by_status(status)
    }

    fn claim_outbox_messages(
        &self,
        worker_id: &str,
        max: usize,
        lease: Duration,
    ) -> Result<Vec<OutboxMessage>, RepositoryError> {
        self.inner
            .repo()
            .inner()
            .claim_outbox_messages(worker_id, max, lease)
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
