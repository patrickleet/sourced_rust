//! Threaded outbox worker for testing distributed systems.
//!
//! This module provides a background thread that drains the outbox
//! and publishes events to a Bus.

#![allow(dead_code)]

use sourced_rust::bus::{Event, Publisher};
use sourced_rust::{HashMapRepository, OutboxRepositoryExt};
use std::sync::mpsc::{channel, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// A background thread that drains outbox messages and publishes to a bus.
///
/// ## Example
///
/// ```ignore
/// let repo = HashMapRepository::new();
/// let bus = InMemoryQueue::new();
///
/// // Start the worker (repo is Clone because it uses Arc<RwLock<...>> internally)
/// let worker = OutboxWorkerThread::spawn(
///     repo.clone(),
///     bus.clone(),
///     Duration::from_millis(50),
/// );
///
/// // ... do work ...
///
/// // Stop the worker
/// worker.stop();
/// ```
pub struct OutboxWorkerThread {
    stop_tx: Sender<()>,
    handle: Option<JoinHandle<WorkerStats>>,
}

/// Statistics from the outbox worker.
#[derive(Debug, Default, Clone)]
pub struct WorkerStats {
    pub messages_published: usize,
    pub messages_failed: usize,
    pub polls: usize,
}

impl OutboxWorkerThread {
    /// Spawn a new outbox worker thread.
    ///
    /// The worker will poll the repository for pending outbox messages,
    /// publish them to the given publisher, and mark them as complete.
    ///
    /// Note: `HashMapRepository` uses `Arc<RwLock<...>>` internally, so cloning
    /// it creates another handle to the same storage - this is thread-safe.
    pub fn spawn<P>(
        repo: HashMapRepository,
        publisher: P,
        poll_interval: Duration,
    ) -> Self
    where
        P: Publisher + 'static,
    {
        Self::spawn_with_id(repo, publisher, poll_interval, "outbox-worker")
    }

    /// Spawn a new outbox worker thread with a custom worker ID.
    pub fn spawn_with_id<P>(
        repo: HashMapRepository,
        publisher: P,
        poll_interval: Duration,
        worker_id: &str,
    ) -> Self
    where
        P: Publisher + 'static,
    {
        let (stop_tx, stop_rx) = channel();
        let worker_id = worker_id.to_string();

        let handle = thread::spawn(move || {
            let mut stats = WorkerStats::default();
            let lease = Duration::from_secs(60);

            loop {
                // Check for stop signal
                match stop_rx.try_recv() {
                    Ok(()) | Err(TryRecvError::Disconnected) => break,
                    Err(TryRecvError::Empty) => {}
                }

                stats.polls += 1;

                // Claim and process messages
                match repo.claim_outbox_messages(&worker_id, 100, lease) {
                    Ok(messages) => {
                        for msg in messages {
                            let event = Event::new(
                                msg.id(),
                                &msg.event_type,
                                msg.payload.clone(),
                            );

                            match publisher.publish(event) {
                                Ok(()) => {
                                    // Mark as complete
                                    if repo.complete_outbox_message(msg.id()).is_ok() {
                                        stats.messages_published += 1;
                                    }
                                }
                                Err(_) => {
                                    // Release for retry
                                    let _ = repo.release_outbox_message(msg.id(), "publish failed");
                                    stats.messages_failed += 1;
                                }
                            }
                        }
                    }
                    Err(_) => {
                        // Repository error, continue polling
                    }
                }

                thread::sleep(poll_interval);
            }

            stats
        });

        Self {
            stop_tx,
            handle: Some(handle),
        }
    }

    /// Signal the worker to stop and wait for it to finish.
    /// Returns the worker statistics.
    pub fn stop(mut self) -> WorkerStats {
        let _ = self.stop_tx.send(());
        if let Some(handle) = self.handle.take() {
            handle.join().unwrap_or_default()
        } else {
            WorkerStats::default()
        }
    }

    /// Signal the worker to stop without waiting.
    pub fn signal_stop(&self) {
        let _ = self.stop_tx.send(());
    }
}

impl Drop for OutboxWorkerThread {
    fn drop(&mut self) {
        let _ = self.stop_tx.send(());
        // Don't join on drop - let the thread finish naturally
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::support::in_memory_queue::InMemoryQueue;
    use sourced_rust::bus::Subscriber;
    use sourced_rust::{Commit, OutboxMessage};

    #[test]
    fn worker_publishes_outbox_messages() {
        // HashMapRepository uses Arc<RwLock<...>> internally, so cloning
        // creates another handle to the same storage
        let repo = HashMapRepository::new();
        let queue = InMemoryQueue::new();

        // Start the worker with a clone of the repo
        let worker = OutboxWorkerThread::spawn(
            repo.clone(),
            queue.clone(),
            Duration::from_millis(10),
        );

        // Create an outbox message directly in the repo
        {
            let mut outbox = OutboxMessage::create(
                "test-msg-1",
                "TestEvent",
                br#"{"data": "hello"}"#.to_vec(),
            );
            repo.commit(&mut outbox.entity).unwrap();
        }

        // Wait for the worker to process
        thread::sleep(Duration::from_millis(100));

        // Stop the worker and get stats
        let stats = worker.stop();

        // Verify the message was published
        assert!(stats.messages_published >= 1);
        assert_eq!(queue.len(), 1);

        let event = queue.poll(10).unwrap().unwrap();
        assert_eq!(event.event_type, "TestEvent");
    }
}
