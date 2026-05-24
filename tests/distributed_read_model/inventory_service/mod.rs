//! Inventory write service: reserves stock when the saga starts and releases it
//! on compensation. It reacts to saga domain events and publishes inventory
//! domain events after updating stock.

pub mod models;

mod handlers;
mod service;

use sourced_rust::{AggregateRepository, HashMapRepository, QueuedRepository};

pub use models::Inventory;
pub use service::{seed_stock, service};

pub type InventoryRepo = AggregateRepository<QueuedRepository<HashMapRepository>, Inventory>;
