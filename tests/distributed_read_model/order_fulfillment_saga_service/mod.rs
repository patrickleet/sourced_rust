//! Order fulfillment saga orchestrator. A `microsvc::Service` whose command
//! handlers mutate the saga model and publish saga domain events. Inventory,
//! payment, order, and projection services react to those events; the saga also
//! reacts to inventory/payment domain events to advance its own state.

pub mod models;

mod handlers;
mod service;

use sourced_rust::{AggregateRepository, HashMapRepository, QueuedRepository};

pub use models::OrderFulfillmentSaga;
pub use service::service;

pub type SagaRepo = AggregateRepository<QueuedRepository<HashMapRepository>, OrderFulfillmentSaga>;
