//! Outbox Worker - Drains and publishes outbox messages.
//!
//! This module provides the worker infrastructure for processing outbox messages:
//! - `OutboxRepositoryExt` - Repository operations for claiming and completing messages
//! - `OutboxWorker` - Synchronous message processor
//! - `OutboxPublisher` - Trait for publishing to external systems
//! - `LogPublisher` - Simple logging publisher for testing
//! - `LocalEmitterPublisher` - In-process event emitter (requires `emitter` feature)
//!
//! ## Separation of Concerns
//!
//! The outbox pattern has two distinct phases:
//! 1. **Commit phase** (see `outbox` module) - Atomically commit aggregate + outbox message
//! 2. **Worker phase** (this module) - Drain outbox and publish to external systems
//!
//! ## Example
//!
//! ```ignore
//! use sourced_rust::{OutboxWorker, OutboxRepositoryExt, LogPublisher};
//!
//! // Claim pending messages
//! let messages = repo.claim_outbox_messages("worker-1", 10, Duration::from_secs(60))?;
//!
//! // Process with a worker
//! let mut worker = OutboxWorker::new(LogPublisher::default());
//! for msg in messages {
//!     worker.process_message(&mut msg);
//!     repo.complete_outbox_message(msg.id())?;
//! }
//! ```

mod publisher;
mod repository_ext;
mod worker;

// Publishers
pub use publisher::{LogPublisher, LogPublisherError, OutboxPublisher};
#[cfg(feature = "emitter")]
pub use publisher::LocalEmitterPublisher;

// Repository helpers
pub use repository_ext::OutboxRepositoryExt;

// Worker
pub use worker::{DrainResult, OutboxWorker, ProcessOneResult};
