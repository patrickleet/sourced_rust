use std::time::Duration;
use std::fmt::Display;

use crate::error::RepositoryError;
use crate::outbox::OutboxRecord;
use crate::queued::QueuedRepository;

pub trait OutboxRepository {
    /// Returns the current outbox contents without claiming or updating status.
    fn peek_outbox(&self) -> Result<Vec<OutboxRecord>, RepositoryError>;
    /// Claims up to `max` outbox records for a worker and sets a lease.
    /// Implementations should return records in a deterministic order (e.g., by id).
    fn claim_outbox(
        &self,
        worker_id: &str,
        max: usize,
        lease: Duration,
    ) -> Result<Vec<OutboxRecord>, RepositoryError>;
    /// Marks outbox records as published.
    fn complete_outbox(&self, ids: &[u64]) -> Result<(), RepositoryError>;
    /// Releases outbox records back to pending and optionally records an error.
    fn release_outbox(&self, ids: &[u64], error: Option<&str>) -> Result<(), RepositoryError>;
    /// Marks outbox records as failed (dead-letter) and optionally records an error.
    fn fail_outbox(&self, ids: &[u64], error: Option<&str>) -> Result<(), RepositoryError>;
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct OutboxDeliveryResult {
    pub claimed: usize,
    pub completed: usize,
    pub released: usize,
    pub failed: usize,
}

pub trait OutboxDelivery: OutboxRepository {
    /// Claim and attempt to deliver. On failure, records are released back to pending.
    fn attempt_outbox<F, E>(
        &self,
        worker_id: &str,
        max: usize,
        lease: Duration,
        mut handler: F,
    ) -> Result<OutboxDeliveryResult, RepositoryError>
    where
        F: FnMut(&OutboxRecord) -> Result<(), E>,
        E: Display,
    {
        let batch = self.claim_outbox(worker_id, max, lease)?;
        let mut result = OutboxDeliveryResult {
            claimed: batch.len(),
            ..Default::default()
        };

        for record in &batch {
            match handler(record) {
                Ok(()) => {
                    self.complete_outbox(&[record.id])?;
                    result.completed += 1;
                }
                Err(err) => {
                    let msg = err.to_string();
                    self.release_outbox(&[record.id], Some(&msg))?;
                    result.released += 1;
                }
            }
        }

        Ok(result)
    }

    /// Claim and deliver with retry limits. When attempts exceed `max_attempts`, records are failed.
    fn deliver_outbox<F, E>(
        &self,
        worker_id: &str,
        max: usize,
        lease: Duration,
        max_attempts: u32,
        mut handler: F,
    ) -> Result<OutboxDeliveryResult, RepositoryError>
    where
        F: FnMut(&OutboxRecord) -> Result<(), E>,
        E: Display,
    {
        let batch = self.claim_outbox(worker_id, max, lease)?;
        let mut result = OutboxDeliveryResult {
            claimed: batch.len(),
            ..Default::default()
        };

        for record in &batch {
            match handler(record) {
                Ok(()) => {
                    self.complete_outbox(&[record.id])?;
                    result.completed += 1;
                }
                Err(err) => {
                    let msg = err.to_string();
                    let should_fail = max_attempts == 0 || record.attempts >= max_attempts;
                    if should_fail {
                        self.fail_outbox(&[record.id], Some(&msg))?;
                        result.failed += 1;
                    } else {
                        self.release_outbox(&[record.id], Some(&msg))?;
                        result.released += 1;
                    }
                }
            }
        }

        Ok(result)
    }
}

impl<T: OutboxRepository> OutboxDelivery for T {}

pub trait Outboxable: OutboxRepository + Sized {
    /// Fluent no-op that expresses outbox capability in builder-style chains.
    fn with_outbox(self) -> Self {
        self
    }
}

impl<T: OutboxRepository> Outboxable for T {}

impl<R: OutboxRepository> OutboxRepository for QueuedRepository<R> {
    fn peek_outbox(&self) -> Result<Vec<OutboxRecord>, RepositoryError> {
        self.inner().peek_outbox()
    }

    fn claim_outbox(
        &self,
        worker_id: &str,
        max: usize,
        lease: Duration,
    ) -> Result<Vec<OutboxRecord>, RepositoryError> {
        self.inner().claim_outbox(worker_id, max, lease)
    }

    fn complete_outbox(&self, ids: &[u64]) -> Result<(), RepositoryError> {
        self.inner().complete_outbox(ids)
    }

    fn release_outbox(&self, ids: &[u64], error: Option<&str>) -> Result<(), RepositoryError> {
        self.inner().release_outbox(ids, error)
    }

    fn fail_outbox(&self, ids: &[u64], error: Option<&str>) -> Result<(), RepositoryError> {
        self.inner().fail_outbox(ids, error)
    }
}
