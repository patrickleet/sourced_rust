use sourced_rust::{
    AggregateBuilder, AggregateRepository, HashMapRepository, OutboxRepository, Queueable,
    QueuedRepository, WithOutbox,
};

use super::todo::Todo;

pub struct TodoRepository {
    inner: AggregateRepository<QueuedRepository<OutboxRepository<HashMapRepository>>, Todo>,
}

impl TodoRepository {
    pub fn new() -> Self {
        TodoRepository {
            inner: HashMapRepository::new()
                .with_outbox()
                .queued()
                .aggregate::<Todo>(),
        }
    }

    /// Access the outbox for processing messages.
    pub fn outbox(&self) -> impl std::ops::Deref<Target = sourced_rust::Outbox> + '_ {
        self.inner.repo().inner().outbox()
    }

    /// Access the outbox mutably for processing messages.
    pub fn outbox_mut(&self) -> impl std::ops::DerefMut<Target = sourced_rust::Outbox> + '_ {
        self.inner.repo().inner().outbox_mut()
    }
}

impl std::ops::Deref for TodoRepository {
    type Target = AggregateRepository<QueuedRepository<OutboxRepository<HashMapRepository>>, Todo>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl std::ops::DerefMut for TodoRepository {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}
