//! Outbox - Atomic commit of aggregates with publishable outbox messages.
//!
//! This module provides the outbox message entity and commit helpers:
//! - `OutboxMessage` - Event-sourced outbox message entity
//! - `OutboxMessageStatus` - Message status (Pending, InFlight, Published, Failed)
//! - `OutboxCommit` - Helper for aggregate + outbox commits
//! - `OutboxCommitExt` - Extension trait for repositories
//!
//! Outbox messages are durable publication work items. Their payload can be a
//! domain event, integration event, command, or generic transport message.
//! Aggregate `EventRecord`s are replayable model history and are not
//! automatically published as domain events.
//!
//! ## Separation of Concerns
//!
//! The outbox pattern has two distinct phases:
//! 1. **Commit phase** (this module) - Atomically commit aggregate event records + outbox message
//! 2. **Worker phase** (see `outbox_worker` module) - Drain outbox and publish messages to external systems
//!
//! Outbox messages are explicit publication records. Aggregate event records are
//! replayable write-side history; they do not automatically become domain or
//! integration events until application code creates an `OutboxMessage` for that
//! publication.
//!
//! ## Example
//!
//! ```ignore
//! use sourced_rust::{OutboxMessage, OutboxCommitExt};
//!
//! // Create aggregate and domain event outbox message
//! let mut order = Order::new();
//! order.create("order-1", ...);
//!
//! let mut outbox = OutboxMessage::create("order-1:created", "OrderCreated", payload);
//!
//! // Commit in one repository batch
//! repo.outbox(&mut outbox).commit(&mut order)?;
//! ```

mod commit;
mod message;

// Event-sourced outbox message
pub use message::{OutboxMessage, OutboxMessageStatus};

// Commit helpers
pub use commit::{OutboxCommit, OutboxCommitExt};
