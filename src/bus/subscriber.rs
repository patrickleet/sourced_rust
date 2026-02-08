//! Core subscriber traits for the service bus.

use super::publisher::{Event, PublishError};

/// Trait for subscribing to events from a message bus.
///
/// This is a pull-based interface. Implementations may also provide
/// push-based interfaces via callbacks or async streams.
pub trait Subscriber: Send + Sync {
    /// Poll for the next event, blocking until one is available or timeout.
    fn poll(&self, timeout_ms: u64) -> Result<Option<Event>, PublishError>;

    /// Acknowledge that an event has been processed.
    fn ack(&self, event_id: &str) -> Result<(), PublishError>;

    /// Reject an event (will be redelivered or sent to dead letter queue).
    fn nack(&self, event_id: &str, reason: &str) -> Result<(), PublishError>;
}

/// Trait for subscribers that can create independent subscriber instances.
///
/// This enables filtered subscriptions via `Bus::subscribe()`.
pub trait Subscribable: Subscriber + Sized {
    /// Create a new independent subscriber sharing the same event source.
    ///
    /// The new subscriber has its own read position, allowing multiple
    /// independent consumers of the same event stream.
    fn new_subscriber(&self) -> Self;
}
