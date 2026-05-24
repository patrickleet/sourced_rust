//! Payment write service: charges after the saga records reserved inventory,
//! declining over a cap to drive the saga's compensation path. It publishes
//! payment domain events after updating the payment model.

pub mod models;

mod handlers;
mod service;

use sourced_rust::{AggregateRepository, HashMapRepository, QueuedRepository};

pub use models::Payment;
pub use service::service;

pub type PaymentRepo = AggregateRepository<QueuedRepository<HashMapRepository>, Payment>;
