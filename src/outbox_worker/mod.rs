//! Outbox Worker - Drains and publishes outbox messages.
//!
//! This module provides the worker infrastructure for processing outbox messages.
//!
//! Items:
//! - `OutboxStore` - Store operations for claiming and completing messages
//! - `OutboxDispatcher` / `BusPublisher` - the async production drain path
//! - `OutboxSource` - outbox-backed durable receive
//! - `BusOutboxPublishHook` - after-commit immediate publish hook
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
//! use distributed::OutboxDispatcher;
//! use std::time::Duration;
//!
//! let dispatcher =
//!     OutboxDispatcher::new(outbox, publisher, "worker-1", Duration::from_secs(60), 3);
//! let outcome = dispatcher.dispatch_batch(10).await?;
//! ```

mod bus_publisher;
mod outbox_dispatch;
mod outbox_source;
mod publish_hook;
mod store;
#[cfg(test)]
mod testing;

// Repository helpers
#[cfg(any(feature = "postgres", feature = "sqlite"))]
pub(crate) use store::ensure_active_claim;
pub use store::{
    ClaimOutboxMessages, OutboxBacklogStats, OutboxClaimRef, OutboxPublishFailureAction,
    OutboxStore, BACKLOG_STATS_SCAN_LIMIT,
};

// Outbox -> bus bridge (moved out of the bus module; depends up on bus traits).
pub use bus_publisher::BusPublisher;
pub use outbox_dispatch::{OutboxDispatchOutcome, OutboxDispatcher, SOURCED_METADATA_PREFIX};
pub use outbox_source::{
    OutboxSource, ReceivedOutboxMessage, DEFAULT_OUTBOX_SOURCE_BATCH, DEFAULT_OUTBOX_SOURCE_LEASE,
};
pub use publish_hook::BusOutboxPublishHook;
