//! Outbox - Atomic commit of aggregates with outbox messages.
//!
//! This module provides the outbox message entity and commit helpers:
//! - `OutboxMessage` - Event-sourced outbox message entity
//! - `OutboxMessageStatus` - Message status (Pending, InFlight, Published, Failed)
//! - `OutboxCommit` - Helper for atomic aggregate + outbox commit
//! - `OutboxCommitExt` - Extension trait for repositories
//!
//! ## Separation of Concerns
//!
//! The outbox pattern has two distinct phases:
//! 1. **Commit phase** (this module) - Atomically commit aggregate + outbox message
//! 2. **Worker phase** (see `outbox_worker` module) - Drain outbox and publish to external systems
//!
//! ## Example
//!
//! ```ignore
//! use sourced_rust::{OutboxMessage, OutboxCommitExt};
//!
//! // Create aggregate and outbox message
//! let mut order = Order::new();
//! order.create("order-1", ...);
//!
//! let mut outbox = OutboxMessage::create("order-1:created", "OrderCreated", payload);
//!
//! // Commit atomically
//! repo.outbox(&mut outbox).commit(&mut order)?;
//! ```

mod commit;
mod message;

// Event-sourced outbox message
pub use message::{OutboxMessage, OutboxMessageStatus};

// Commit helpers
pub use commit::{OutboxCommit, OutboxCommitExt};
