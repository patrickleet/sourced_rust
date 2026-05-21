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
//! use std::time::Duration;
//!
//! // Claim pending messages
//! let worker_id = "worker-1";
//! let messages = repo.claim_outbox_messages(worker_id, 10, Duration::from_secs(60))?;
//!
//! // Process with a worker
//! let mut worker = OutboxWorker::new(LogPublisher::default()).with_worker_id(worker_id);
//! for mut msg in messages {
//!     let result = worker.process_message(&mut msg)?;
//!     if result.completed {
//!         repo.complete_outbox_message_for_worker(msg.id(), worker_id)?;
//!     } else if result.released || result.failed {
//!         let error = msg.last_error.as_deref().unwrap_or("publish failed");
//!         repo.record_outbox_publish_failure(msg.id(), worker_id, error, 3)?;
//!     }
//! }
//! ```

mod publisher;
mod repository_ext;
#[cfg(feature = "bus")]
mod thread;
mod worker;

// Publishers
#[cfg(feature = "emitter")]
pub use publisher::LocalEmitterPublisher;
pub use publisher::{LogPublisher, LogPublisherError, OutboxPublisher};

// Repository helpers
pub use repository_ext::{OutboxPublishFailureAction, OutboxRepositoryExt};

// Worker
pub use worker::{DrainResult, OutboxWorker, ProcessOneResult};

// Threaded worker (requires bus feature)
#[cfg(feature = "bus")]
pub use thread::{OutboxWorkerJoinError, OutboxWorkerThread, WorkerStats};
