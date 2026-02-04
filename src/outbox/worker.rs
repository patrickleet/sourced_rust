use std::time::Duration;

use super::aggregate::DomainEvent;
use super::publisher::OutboxPublisher;

/// Result of a batch drain operation.
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

/// Worker for processing outbox messages.
///
/// The repository is responsible for claiming pending messages. The worker
/// processes messages that are already loaded and (optionally) claimed.
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

    /// Set the batch size (max records to process per drain).
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
    /// Process a single outbox message.
    ///
    /// If the message is pending, it will be claimed by this worker before
    /// publishing. The caller is responsible for persisting the updated
    /// message entity after processing.
    pub fn process_message(&mut self, message: &mut DomainEvent) -> ProcessOneResult {
        if message.is_published() || message.is_failed() {
            return ProcessOneResult::default();
        }

        if message.is_pending() {
            message.claim(&self.worker_id, self.lease);
        }

        if !message.is_in_flight() {
            return ProcessOneResult::default();
        }

        match self.publisher.publish(&message.event_type, &message.payload) {
            Ok(()) => {
                message.complete();
                ProcessOneResult {
                    did_work: true,
                    completed: true,
                    ..Default::default()
                }
            }
            Err(err) => {
                let error_msg = err.to_string();
                if message.attempts >= self.max_attempts {
                    message.fail(Some(&error_msg));
                    ProcessOneResult {
                        did_work: true,
                        failed: true,
                        ..Default::default()
                    }
                } else {
                    message.release(Some(&error_msg));
                    ProcessOneResult {
                        did_work: true,
                        released: true,
                        ..Default::default()
                    }
                }
            }
        }
    }

    /// Process a batch of outbox messages.
    pub fn process_batch(&mut self, messages: &mut [DomainEvent]) -> DrainResult {
        let mut result = DrainResult::default();

        for message in messages.iter_mut().take(self.batch_size) {
            let processed = self.process_message(message);
            if processed.did_work {
                result.claimed += 1;
            }
            if processed.completed {
                result.completed += 1;
            }
            if processed.released {
                result.released += 1;
            }
            if processed.failed {
                result.failed += 1;
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
    fn process_message_noop_for_published() {
        let mut message = DomainEvent::new("msg-1", "Event", "{}");
        message.claim("worker", Duration::from_secs(1));
        message.complete();

        let mut worker = OutboxWorker::new(LogPublisher::default());
        let result = worker.process_message(&mut message);
        assert!(!result.did_work);
    }
}
