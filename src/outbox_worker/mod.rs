//! Outbox Worker - Drains and publishes outbox messages.
//!
//! This module provides the worker infrastructure for processing outbox messages.
//!
//! Items:
//! - `AsyncOutboxStore` - Store operations for claiming and completing messages
//! - `OutboxDispatcher` / `BusPublisher` - the async production drain path
//! - `OutboxWorker` - synchronous dev/test message processor
//! - `OutboxPublisher` - synchronous dev/test publish trait
//! - `LogPublisher` - simple logging publisher for tests
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
//! use distributed::{AsyncOutboxStore, ClaimOutboxMessages, OutboxClaimRef};
//! use std::time::Duration;
//!
//! let worker_id = "worker-1";
//! let messages = outbox
//!     .claim_async(ClaimOutboxMessages::new(worker_id, 10, Duration::from_secs(60)))
//!     .await?;
//! for msg in messages {
//!     let claim = OutboxClaimRef::from_message(&msg)?;
//!     outbox.complete_async(&claim).await?;
//! }
//! ```

mod bus_publisher;
mod outbox_dispatch;
mod outbox_source;
mod publish_hook;
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
    AsyncOutboxStore, ClaimOutboxMessages, OutboxClaimRef, OutboxPublishFailureAction,
};

// Worker
pub use worker::{DrainResult, OutboxWorker, ProcessOneResult};

// Outbox -> bus bridge (moved out of the bus module; depends up on bus traits).
pub use bus_publisher::BusPublisher;
pub use outbox_dispatch::{OutboxDispatchOutcome, OutboxDispatcher, SOURCED_METADATA_PREFIX};
pub use outbox_source::{
    OutboxSource, ReceivedOutboxMessage, DEFAULT_OUTBOX_SOURCE_BATCH, DEFAULT_OUTBOX_SOURCE_LEASE,
};
pub use publish_hook::BusOutboxPublishHook;
