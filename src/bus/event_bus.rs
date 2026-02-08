//! Combined trait for bidirectional bus communication.

use super::publisher::Publisher;
use super::subscriber::Subscriber;

/// Combined trait for bidirectional bus communication.
pub trait EventBus: Publisher + Subscriber {}

// Blanket implementation
impl<T: Publisher + Subscriber> EventBus for T {}
