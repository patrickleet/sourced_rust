//! Service Bus - Event publishing abstractions
//!
//! This module provides traits and implementations for publishing events
//! to various message brokers and event buses.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    Bus (per service)                         │
//! │  - Wraps Publisher + Subscriber                             │
//! │  - publish() / poll() / ack()                               │
//! └─────────────────────────────────────────────────────────────┘
//!                            │
//!                            ▼
//! ┌─────────────────────────────────────────────────────────────┐
//! │              Publisher + Subscriber Traits                   │
//! │  Publisher: publish(event) / publish_batch(events)          │
//! │  Subscriber: poll(timeout) / ack(id) / nack(id)             │
//! └─────────────────────────────────────────────────────────────┘
//!          │                  │                     │
//!          ▼                  ▼                     ▼
//! ┌─────────────┐    ┌─────────────┐    ┌─────────────────────┐
//! │InMemoryQueue│    │ KafkaQueue  │    │ RedisStreamQueue    │
//! │ (included)  │    │ (external)  │    │    (external)       │
//! └─────────────┘    └─────────────┘    └─────────────────────┘
//! ```
//!
//! ## Usage with Outbox Pattern
//!
//! ```ignore
//! // 1. Commit aggregate with outbox message
//! repo.outbox(&mut outbox_msg).commit(&mut order)?;
//!
//! // 2. Worker drains outbox and publishes via bus
//! let bus = Bus::new(kafka_publisher, kafka_subscriber);
//! for msg in repo.claim_outbox_messages(...) {
//!     let event = Event::new(msg.id(), &msg.event_type, msg.payload.clone());
//!     bus.publish(event)?;
//! }
//! ```

mod bus;
mod event_bus;
mod in_memory_queue;
mod listener;
mod publisher;
mod sender;
mod subscriber;

pub use bus::Bus;
pub use event_bus::EventBus;
pub use in_memory_queue::{EventReceiver, InMemoryQueue};
pub use listener::Listener;
pub use publisher::{Event, PublishError, Publisher};
pub use sender::Sender;
pub use subscriber::{Subscribable, Subscriber};

/// Type alias for `Event` when used in a command/message context.
///
/// Commands and events are both messages — the distinction is in how they're
/// routed: `publish/subscribe` = events (fan-out), `send/listen` = commands
/// (point-to-point). This alias makes command handler signatures read naturally.
pub type Message = Event;
