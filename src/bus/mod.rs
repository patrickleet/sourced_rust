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
//! │ (tests only)│    │ (external)  │    │    (external)       │
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
mod publisher;

pub use bus::Bus;
pub use publisher::{Event, EventBus, PublishError, Publisher, Subscriber};
