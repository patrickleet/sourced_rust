//! Background thread for a domain service listening on a bus queue.
//!
//! `DomainServiceThread` spawns a background thread that listens for
//! messages on a named queue (point-to-point) and dispatches them to
//! handlers registered on a `DomainService`.

use std::sync::mpsc::{channel, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::bus::Listener;
use super::domain_service::DomainService;

/// Statistics from the domain service thread.
#[derive(Debug, Default, Clone)]
pub struct ServiceStats {
    /// Number of messages successfully handled.
    pub messages_handled: usize,
    /// Number of messages that failed handling.
    pub messages_failed: usize,
    /// Number of poll cycles completed.
    pub polls: usize,
}

/// A background thread that listens on a named queue and dispatches
/// messages to a `DomainService`.
///
/// Follows the same pattern as `OutboxWorkerThread`: spawn, do work,
/// stop and collect stats.
///
/// ## Example
///
/// ```ignore
/// use sourced_rust::{DomainService, DomainServiceThread, HashMapRepository};
/// use sourced_rust::bus::InMemoryQueue;
/// use std::time::Duration;
///
/// let queue = InMemoryQueue::new();
/// let repo = HashMapRepository::new();
///
/// let service = DomainService::new(repo)
///     .command("order.create", |repo, msg| { Ok(()) });
///
/// let handle = DomainServiceThread::spawn(
///     service,
///     "orders".to_string(),
///     queue.clone(),
///     Duration::from_millis(10),
/// );
///
/// // ... send commands to the queue ...
///
/// let stats = handle.stop();
/// println!("Handled {} messages", stats.messages_handled);
/// ```
pub struct DomainServiceThread {
    stop_tx: Sender<()>,
    handle: Option<JoinHandle<ServiceStats>>,
}

impl DomainServiceThread {
    /// Spawn a service that listens on a named queue (point-to-point commands).
    pub fn spawn<R, L>(
        service: DomainService<R>,
        queue_name: String,
        listener: L,
        poll_interval: Duration,
    ) -> Self
    where
        R: Send + Sync + 'static,
        L: Listener + 'static,
    {
        let (stop_tx, stop_rx) = channel();

        let handle = thread::spawn(move || {
            let mut stats = ServiceStats::default();

            loop {
                // Check for stop signal
                match stop_rx.try_recv() {
                    Ok(()) | Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                }

                stats.polls += 1;

                match listener.listen(&queue_name, poll_interval.as_millis() as u64) {
                    Ok(Some(message)) => match service.handle(&message) {
                        Ok(()) => {
                            stats.messages_handled += 1;
                        }
                        Err(_e) => {
                            stats.messages_failed += 1;
                        }
                    },
                    Ok(None) => {
                        // No message available, continue polling
                    }
                    Err(_) => {
                        // Listener error, continue polling
                    }
                }
            }

            stats
        });

        Self {
            stop_tx,
            handle: Some(handle),
        }
    }

    /// Signal the service to stop and wait for it to finish.
    /// Returns the service statistics.
    pub fn stop(mut self) -> ServiceStats {
        let _ = self.stop_tx.send(());
        if let Some(handle) = self.handle.take() {
            handle.join().unwrap_or_default()
        } else {
            ServiceStats::default()
        }
    }

    /// Signal the service to stop without waiting.
    pub fn signal_stop(&self) {
        let _ = self.stop_tx.send(());
    }
}

impl Drop for DomainServiceThread {
    fn drop(&mut self) {
        let _ = self.stop_tx.send(());
    }
}
