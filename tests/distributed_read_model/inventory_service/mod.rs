//! Inventory write service: reserves stock for the saga and releases it on
//! compensation. A `microsvc::Service` whose handlers react to
//! `fulfillment.reserve_inventory` / `fulfillment.release_inventory` and publish
//! the result through the outbox — same shape as every other service.

pub mod models;

mod handlers;
mod service;

use sourced_rust::{AggregateRepository, HashMapRepository, QueuedRepository};

pub use models::Inventory;
pub use service::{model_service, seed_stock};

pub type InventoryRepo = AggregateRepository<QueuedRepository<HashMapRepository>, Inventory>;
