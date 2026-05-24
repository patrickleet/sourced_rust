//! Catalog write service: owns the `Product` aggregate and publishes product
//! snapshots through its outbox. It shares nothing with the order service except
//! the bus.

pub mod models;

mod handlers;
mod service;

use sourced_rust::{AggregateRepository, HashMapRepository, QueuedRepository};

pub use models::{AddProduct, Product, ProductSnapshot, RepriceProduct};
pub use service::model_service;

pub type CatalogRepo = AggregateRepository<QueuedRepository<HashMapRepository>, Product>;
