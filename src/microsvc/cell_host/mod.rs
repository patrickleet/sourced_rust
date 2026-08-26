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

pub(crate) mod causal;
mod cell;
#[cfg(feature = "graphql")]
mod command;
mod internal_auth;
#[cfg(feature = "graphql")]
mod outbox;
mod store;
mod wire;

pub use causal::{
    CellCommandIdentity, CellDispatchError, CellDispatchResult, CELL_PRINCIPAL_PARTITION_HEADER,
    CELL_SERVICE_ID_HEADER,
};
pub use cell::{instance_name, parent_cell_name, AggregateCell, CellNamespace};
#[cfg(feature = "graphql")]
pub use command::{CelldCommandHost, CelldRoute};
pub use internal_auth::{
    InternalHttpSecret, CELL_INTERNAL_SECRET_ENV, CELL_INTERNAL_SECRET_HEADER,
};
#[cfg(feature = "graphql")]
pub use outbox::{
    accept_outbox_drain, outbox_alarm_handler, CellOutboxDrainHandler, CellOutboxScheduler,
    CELL_OUTBOX_DRAIN_PATH,
};
pub use store::{CellStreamStore, DurableCellCommand, DurableCellEvents, DurableCellSnapshot};
pub(crate) use wire::validate_cell_outbox_messages;
pub use wire::{
    parse_cell_outbox, parse_claimed_cell_outbox, CellOutboxHint, CellOutboxWireItem,
    CellWaitPathRequest, MAX_CELL_OUTBOX_ITEMS, MAX_CELL_OUTBOX_PAYLOAD_BYTES,
    MAX_CELL_OUTBOX_WIRE_BYTES,
};

#[cfg(test)]
mod tests;
