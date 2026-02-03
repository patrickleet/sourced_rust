use std::time::Duration;
use std::thread;

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

    pub fn drain_loop<F>(
        &mut self,
        worker_id: &str,
        max: usize,
        lease: Duration,
        max_attempts: u32,
        idle_delay: Duration,
        max_idle_delay: Duration,
        mut should_stop: F,
    ) -> Result<(), RepositoryError>
    where
        F: FnMut() -> bool,
    {
        let mut delay = idle_delay;
        let max_idle_delay = if max_idle_delay < idle_delay {
            idle_delay
        } else {
            max_idle_delay
        };

        while !should_stop() {
            let result = self.drain_once(worker_id, max, lease, max_attempts)?;
            if result.claimed == 0 {
                thread::sleep(delay);
                let next = delay.checked_mul(2).unwrap_or(max_idle_delay);
                delay = std::cmp::min(next, max_idle_delay);
            } else {
                delay = idle_delay;
            }
        }

        Ok(())
    }
}
