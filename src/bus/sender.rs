//! Sender trait for point-to-point messaging.

use super::publisher::{Event, PublishError};

/// Trait for sending events to a named queue (point-to-point).
///
/// Unlike `Publisher` (fan-out to all subscribers), `Sender` delivers
/// messages to a specific named queue where only one listener consumes
/// each message (competing consumers).
pub trait Sender: Send + Sync {
    /// Send an event to a named queue.
    fn send(&self, queue: &str, event: Event) -> Result<(), PublishError>;
}
