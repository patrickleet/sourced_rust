//! Threaded outbox worker for background message processing.
//!
//! This module provides a background thread that drains the outbox
//! and publishes events to a message bus.

use std::sync::mpsc::{channel, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::bus::{Event, Publisher, Sender as BusSender};
use crate::OutboxRepositoryExt;

/// Statistics from the outbox worker.
#[derive(Debug, Default, Clone)]
pub struct WorkerStats {
    pub messages_published: usize,
    pub messages_failed: usize,
    pub polls: usize,
}

/// A background thread that drains outbox messages and publishes to a bus.
///
/// ## Example
///
/// ```ignore
/// use sourced_rust::{HashMapRepository, OutboxWorkerThread};
/// use sourced_rust::bus::Publisher;
/// use std::time::Duration;
///
/// let repo = HashMapRepository::new();
/// let publisher = MyPublisher::new(); // implements Publisher
///
/// // Start the worker
/// let worker = OutboxWorkerThread::spawn(
///     repo.clone(),
///     publisher,
///     Duration::from_millis(50),
/// );
///
/// // ... do work ...
///
/// // Stop the worker and get stats
/// let stats = worker.stop();
/// println!("Published {} messages", stats.messages_published);
/// ```
pub struct OutboxWorkerThread {
    stop_tx: Sender<()>,
    handle: Option<JoinHandle<WorkerStats>>,
}

impl OutboxWorkerThread {
    /// Spawn a new outbox worker thread.
    ///
    /// The worker will poll the repository for pending outbox messages,
    /// publish them to the given publisher, and mark them as complete.
    ///
    /// The repository must be `Clone + Send + 'static`. For `HashMapRepository`,
    /// cloning creates another handle to the same storage (thread-safe via `Arc<RwLock<...>>`).
    pub fn spawn<R, P>(repo: R, publisher: P, poll_interval: Duration) -> Self
    where
        R: OutboxRepositoryExt + Clone + Send + 'static,
        P: Publisher + 'static,
    {
        Self::spawn_with_id(repo, publisher, poll_interval, "outbox-worker")
    }

    /// Spawn a new outbox worker thread with a custom worker ID.
    pub fn spawn_with_id<R, P>(
        repo: R,
        publisher: P,
        poll_interval: Duration,
        worker_id: &str,
    ) -> Self
    where
        R: OutboxRepositoryExt + Clone + Send + 'static,
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
                            let event =
                                Event::new(msg.id(), &msg.event_type, msg.payload.clone());

                            match publisher.publish(event) {
                                Ok(()) => {
                                    // Mark as complete
                                    if repo.complete_outbox_message(msg.id()).is_ok() {
                                        stats.messages_published += 1;
                                    }
                                }
                                Err(_) => {
                                    // Release for retry
                                    let _ =
                                        repo.release_outbox_message(msg.id(), "publish failed");
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

    /// Spawn a worker that routes messages based on their destination.
    ///
    /// Messages with a `destination` are sent point-to-point via `Sender::send()`.
    /// Messages without a destination are published fan-out via `Publisher::publish()`.
    pub fn spawn_routed<R, P>(repo: R, publisher: P, poll_interval: Duration) -> Self
    where
        R: OutboxRepositoryExt + Clone + Send + 'static,
        P: Publisher + BusSender + 'static,
    {
        Self::spawn_routed_with_id(repo, publisher, poll_interval, "outbox-worker")
    }

    /// Spawn a routed worker with a custom worker ID.
    pub fn spawn_routed_with_id<R, P>(
        repo: R,
        publisher: P,
        poll_interval: Duration,
        worker_id: &str,
    ) -> Self
    where
        R: OutboxRepositoryExt + Clone + Send + 'static,
        P: Publisher + BusSender + 'static,
    {
        let (stop_tx, stop_rx) = channel();
        let worker_id = worker_id.to_string();

        let handle = thread::spawn(move || {
            let mut stats = WorkerStats::default();
            let lease = Duration::from_secs(60);

            loop {
                match stop_rx.try_recv() {
                    Ok(()) | Err(TryRecvError::Disconnected) => break,
                    Err(TryRecvError::Empty) => {}
                }

                stats.polls += 1;

                match repo.claim_outbox_messages(&worker_id, 100, lease) {
                    Ok(messages) => {
                        for msg in messages {
                            let event =
                                Event::new(msg.id(), &msg.event_type, msg.payload.clone());

                            let result = if let Some(dest) = &msg.destination {
                                publisher.send(dest, event)
                            } else {
                                publisher.publish(event)
                            };

                            match result {
                                Ok(()) => {
                                    if repo.complete_outbox_message(msg.id()).is_ok() {
                                        stats.messages_published += 1;
                                    }
                                }
                                Err(_) => {
                                    let _ =
                                        repo.release_outbox_message(msg.id(), "publish failed");
                                    stats.messages_failed += 1;
                                }
                            }
                        }
                    }
                    Err(_) => {}
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
