//! Payment write service: charges the order amount, declining over a cap to
//! drive the saga's compensation path. A `microsvc::Service` reacting to
//! `fulfillment.charge_payment`.

pub mod models;

mod handlers;
mod service;

use sourced_rust::{AggregateRepository, HashMapRepository, QueuedRepository};

pub use models::Payment;
pub use service::service;

pub type PaymentRepo = AggregateRepository<QueuedRepository<HashMapRepository>, Payment>;
