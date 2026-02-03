use std::time::Duration;

use crate::error::RepositoryError;

use super::{OutboxDelivery, OutboxDeliveryResult, OutboxRepository};
use super::publisher::OutboxPublisher;

pub struct OutboxWorker<R, P> {
    repo: R,
    publisher: P,
}

impl<R, P> OutboxWorker<R, P> {
    pub fn new(repo: R, publisher: P) -> Self {
        OutboxWorker { repo, publisher }
    }

    pub fn repo(&self) -> &R {
        &self.repo
    }

    pub fn repo_mut(&mut self) -> &mut R {
        &mut self.repo
    }
}

impl<R: OutboxRepository, P: OutboxPublisher> OutboxWorker<R, P> {
    pub fn drain_once(
        &mut self,
        worker_id: &str,
        max: usize,
        lease: Duration,
        max_attempts: u32,
    ) -> Result<OutboxDeliveryResult, RepositoryError> {
        let publisher = &mut self.publisher;
        self.repo.deliver_outbox(worker_id, max, lease, max_attempts, |record| {
            publisher.publish(record)
        })
    }
}
