//! Outbox Worker - Drains and publishes outbox messages.
//!
//! This module provides the worker infrastructure for processing outbox messages:
//! - `OutboxStore` - Store operations for claiming and completing messages
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
//! use distributed::{ClaimOutboxMessages, OutboxClaimRef, OutboxStore, OutboxWorker, LogPublisher};
//! use std::time::Duration;
//!
//! // Claim pending messages
//! let worker_id = "worker-1";
//! let messages = outbox.claim(ClaimOutboxMessages::new(worker_id, 10, Duration::from_secs(60)))?;
//!
//! // Process with a worker
//! let mut worker = OutboxWorker::new(LogPublisher::default()).with_worker_id(worker_id);
//! for mut msg in messages {
//!     let claim = OutboxClaimRef::from_message(&msg)?;
//!     let result = worker.process_message(&mut msg)?;
//!     if result.completed {
//!         outbox.complete(&claim)?;
//!     } else if result.released || result.failed {
//!         let error = msg.last_error.as_deref().unwrap_or("publish failed");
//!         outbox.record_failure(&claim, error, 3)?;
//!     }
//! }
//! ```

mod bus_publisher;
mod outbox_dispatch;
mod outbox_source;
mod publisher;
mod store;
mod worker;

// Publishers
#[cfg(feature = "emitter")]
pub use publisher::LocalEmitterPublisher;
pub use publisher::{LogPublisher, LogPublisherError, OutboxPublisher};

// Repository helpers
#[cfg(any(feature = "postgres", feature = "sqlite"))]
pub(crate) use store::ensure_active_claim;
pub use store::{
    AsyncOutboxStore, ClaimOutboxMessages, OutboxClaimRef, OutboxPublishFailureAction, OutboxStore,
};

// Worker
pub use worker::{DrainResult, OutboxWorker, ProcessOneResult};

// Outbox -> bus bridge (moved out of the bus module; depends up on bus traits).
pub use bus_publisher::BusPublisher;
pub use outbox_dispatch::{OutboxDispatchOutcome, OutboxDispatcher, SOURCED_METADATA_PREFIX};
pub use outbox_source::{
    OutboxSource, ReceivedOutboxMessage, DEFAULT_OUTBOX_SOURCE_BATCH, DEFAULT_OUTBOX_SOURCE_LEASE,
};
