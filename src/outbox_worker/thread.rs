//! Threaded outbox worker for background message processing.
//!
//! This module provides a background thread that drains the outbox
//! and publishes events to a message bus.

use std::sync::mpsc::{channel, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use std::{error::Error, fmt};

use crate::bus::{Event, Publisher, Sender as BusSender};
use crate::OutboxRepositoryExt;

/// Statistics from the outbox worker.
#[derive(Debug, Default, Clone)]
pub struct WorkerStats {
    pub messages_published: usize,
    pub messages_failed: usize,
    pub polls: usize,
}

/// Error returned when an outbox worker thread fails during shutdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutboxWorkerJoinError;

impl fmt::Display for OutboxWorkerJoinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "outbox worker thread panicked during shutdown")
    }
}

impl Error for OutboxWorkerJoinError {}

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
/// let stats = worker.stop()?;
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
                            let mut event =
                                Event::new(msg.id(), &msg.event_type, msg.payload.clone());
                            for (k, v) in &msg.metadata {
                                event = event.with_metadata(k, v);
                            }

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
                            let mut event =
                                Event::new(msg.id(), &msg.event_type, msg.payload.clone());
                            for (k, v) in &msg.metadata {
                                event = event.with_metadata(k, v);
                            }

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
                                    let _ = repo.release_outbox_message(msg.id(), "publish failed");
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
    ///
    /// Returns [`OutboxWorkerJoinError`] if the worker thread panicked before
    /// shutdown completed.
    pub fn stop(mut self) -> Result<WorkerStats, OutboxWorkerJoinError> {
        let _ = self.stop_tx.send(());
        if let Some(handle) = self.handle.take() {
            handle.join().map_err(|_| OutboxWorkerJoinError)
        } else {
            Ok(WorkerStats::default())
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

    #[test]
    fn stop_returns_stats_when_worker_exits_cleanly() {
        let (stop_tx, stop_rx) = channel();
        let handle = thread::spawn(move || {
            let _ = stop_rx.recv();
            WorkerStats {
                messages_published: 2,
                messages_failed: 1,
                polls: 3,
            }
        });
        let worker = OutboxWorkerThread {
            stop_tx,
            handle: Some(handle),
        };

        let stats = worker.stop().unwrap();

        assert_eq!(stats.messages_published, 2);
        assert_eq!(stats.messages_failed, 1);
        assert_eq!(stats.polls, 3);
    }

    #[test]
    fn stop_returns_error_when_worker_thread_panics() {
        let (stop_tx, _stop_rx) = channel();
        let handle = thread::spawn(|| -> WorkerStats {
            panic!("worker panic");
        });
        let worker = OutboxWorkerThread {
            stop_tx,
            handle: Some(handle),
        };

        let err = worker
            .stop()
            .expect_err("worker thread panic should be returned");

        assert_eq!(err, OutboxWorkerJoinError);
    }
}
