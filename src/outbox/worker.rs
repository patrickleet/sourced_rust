use std::time::Duration;

use super::aggregate::Outbox;
use super::publisher::OutboxPublisher;

/// Result of a single drain operation.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DrainResult {
    pub claimed: usize,
    pub completed: usize,
    pub released: usize,
    pub failed: usize,
}

/// Result of processing a single message.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ProcessOneResult {
    /// Whether any work was done (a message was processed).
    pub did_work: bool,
    /// Whether the message was successfully published.
    pub completed: bool,
    /// Whether the message was released for retry.
    pub released: bool,
    /// Whether the message permanently failed.
    pub failed: bool,
}

/// Worker for processing outbox records.
///
/// The worker claims records from an Outbox aggregate, attempts to publish them,
/// and marks them as completed, released, or failed based on the outcome.
///
/// # Example
///
/// ```ignore
/// let worker = OutboxWorker::new(MyPublisher::new())
///     .with_worker_id("worker-1")
///     .with_batch_size(20)
///     .with_max_attempts(5);
///
/// // Get outbox from repository
/// let mut outbox = repo.outbox_mut();
/// let result = worker.drain_once(&mut outbox);
/// // Then commit the outbox to persist state changes
/// ```
pub struct OutboxWorker<P> {
    publisher: P,
    worker_id: String,
    batch_size: usize,
    lease: Duration,
    max_attempts: u32,
}

impl<P> OutboxWorker<P> {
    /// Create a new worker with the given publisher.
    pub fn new(publisher: P) -> Self {
        Self {
            publisher,
            worker_id: format!("worker-{}", std::process::id()),
            batch_size: 10,
            lease: Duration::from_secs(60),
            max_attempts: 3,
        }
    }

    /// Set the worker ID (used for lease tracking).
    pub fn with_worker_id(mut self, id: impl Into<String>) -> Self {
        self.worker_id = id.into();
        self
    }

    /// Set the batch size (max records to claim per drain).
    pub fn with_batch_size(mut self, size: usize) -> Self {
        self.batch_size = size;
        self
    }

    /// Set the lease duration for claimed records.
    pub fn with_lease(mut self, lease: Duration) -> Self {
        self.lease = lease;
        self
    }

    /// Set the maximum number of attempts before failing a record.
    pub fn with_max_attempts(mut self, max: u32) -> Self {
        self.max_attempts = max;
        self
    }

    /// Get a reference to the publisher.
    pub fn publisher(&self) -> &P {
        &self.publisher
    }

    /// Get a mutable reference to the publisher.
    pub fn publisher_mut(&mut self) -> &mut P {
        &mut self.publisher
    }
}

impl<P: OutboxPublisher> OutboxWorker<P> {
    /// Process a single message from the outbox.
    ///
    /// This method:
    /// 1. Claims one pending message
    /// 2. Attempts to publish it
    /// 3. Marks it as completed, released, or failed
    ///
    /// The caller should commit the outbox after each call to persist
    /// the state change immediately. This ensures that if the process
    /// crashes, already-published messages won't be reprocessed.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mut outbox = repo.outbox_mut();
    /// while worker.process_next(&mut outbox).did_work {
    ///     repo.commit_outbox()?;  // persist immediately
    /// }
    /// ```
    pub fn process_next(&mut self, outbox: &mut Outbox) -> ProcessOneResult {
        // Claim just one message
        let claimed_ids = outbox.claim(&self.worker_id, 1, self.lease);
        if claimed_ids.is_empty() {
            return ProcessOneResult::default();
        }

        let id = claimed_ids[0];
        let (event_type, payload, attempts) = {
            let message = outbox.get(id).unwrap();
            (
                message.event_type.clone(),
                message.payload.clone(),
                message.attempts,
            )
        };

        match self.publisher.publish(&event_type, &payload) {
            Ok(()) => {
                outbox.complete(id);
                ProcessOneResult {
                    did_work: true,
                    completed: true,
                    ..Default::default()
                }
            }
            Err(err) => {
                let error_msg = err.to_string();
                if attempts >= self.max_attempts {
                    outbox.fail(id, Some(&error_msg));
                    ProcessOneResult {
                        did_work: true,
                        failed: true,
                        ..Default::default()
                    }
                } else {
                    outbox.release(id, Some(&error_msg));
                    ProcessOneResult {
                        did_work: true,
                        released: true,
                        ..Default::default()
                    }
                }
            }
        }
    }

    /// Process one batch of outbox records.
    ///
    /// This method:
    /// 1. Claims pending records from the outbox
    /// 2. Attempts to publish each record
    /// 3. Marks records as completed, released, or failed
    ///
    /// **Warning**: State changes are only in memory until committed.
    /// If the process crashes before commit, published messages may be
    /// reprocessed. For safer at-least-once delivery, use `process_next`
    /// with commits after each message.
    pub fn drain_once(&mut self, outbox: &mut Outbox) -> DrainResult {
        // Claim records
        let claimed_ids = outbox.claim(&self.worker_id, self.batch_size, self.lease);
        if claimed_ids.is_empty() {
            return DrainResult::default();
        }

        let mut result = DrainResult {
            claimed: claimed_ids.len(),
            ..Default::default()
        };

        // Process each record
        for id in claimed_ids {
            // Get record info before processing
            let (event_type, payload, attempts) = {
                let record = outbox.get(id).unwrap();
                (
                    record.event_type.clone(),
                    record.payload.clone(),
                    record.attempts,
                )
            };

            match self.publisher.publish(&event_type, &payload) {
                Ok(()) => {
                    outbox.complete(id);
                    result.completed += 1;
                }
                Err(err) => {
                    let error_msg = err.to_string();
                    if attempts >= self.max_attempts {
                        outbox.fail(id, Some(&error_msg));
                        result.failed += 1;
                    } else {
                        outbox.release(id, Some(&error_msg));
                        result.released += 1;
                    }
                }
            }
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outbox::LogPublisher;

    #[test]
    fn worker_builder() {
        let worker = OutboxWorker::new(LogPublisher::default())
            .with_worker_id("test-worker")
            .with_batch_size(5)
            .with_lease(Duration::from_secs(30))
            .with_max_attempts(2);

        assert_eq!(worker.worker_id, "test-worker");
        assert_eq!(worker.batch_size, 5);
        assert_eq!(worker.lease, Duration::from_secs(30));
        assert_eq!(worker.max_attempts, 2);
    }

    #[test]
    fn drain_empty_outbox() {
        let mut worker = OutboxWorker::new(LogPublisher::default());
        let mut outbox = Outbox::new("test");

        let result = worker.drain_once(&mut outbox);
        assert_eq!(result.claimed, 0);
        assert_eq!(result.completed, 0);
    }
}
