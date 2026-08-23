//! Second command host: a celld Durable Object class analogue.
//!
//! Domain crates keep `CausalCommandContext` / `ctx.repo()`. This module is
//! the host adapter: one named cell per shard, private stream store, same
//! [`PortableCommand`] mounts as SOA `Routes`. It is **not** a sqlx dialect
//! and must not be gated behind `feature = "celld"` (`PCH-DEC-005`).
//!
//! Live celld fleet / CI is not required (`PCH-AC-006.1`). A workers-rs
//! `Send` tax stays in this adapter; cell types do not leak into domain
//! command declarations.
//!
//! GraphQL wait-path + cell SQLite outbox drain (`CelldCommandHost`) is
//! the same for every aggregate: routes only supply kind, shard, and payload.

mod cell;
mod store;
#[cfg(feature = "graphql")]
mod command;
#[cfg(feature = "graphql")]
mod outbox;

pub use cell::{instance_name, parent_cell_name, AggregateCell, CellNamespace};
pub use store::{CellStreamStore, DurableCellEvents, DurableCellSnapshot};
#[cfg(feature = "graphql")]
pub use command::{CelldCommandHost, CelldRoute};
#[cfg(feature = "graphql")]
pub use outbox::{
    accept_outbox_drain, drain_cell_outbox, outbox_alarm_handler, spawn_cell_outbox_drain_loop,
    CellOutboxDrainHandler, CELL_OUTBOX_DRAIN_PATH,
};

#[cfg(test)]
mod tests;
