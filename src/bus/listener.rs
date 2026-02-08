//! Listener trait for point-to-point messaging.

use super::publisher::{Event, PublishError};

/// Trait for listening on a named queue (point-to-point).
///
/// Unlike `Subscriber` (fan-out where each subscriber sees all events),
/// `Listener` competes with other listeners on the same queue — each
/// message is delivered to exactly one listener.
pub trait Listener: Send + Sync {
    /// Listen for the next event on a named queue, blocking until one
    /// is available or the timeout expires.
    fn listen(&self, queue: &str, timeout_ms: u64) -> Result<Option<Event>, PublishError>;
}
