//! Order fulfillment saga orchestrator. A `microsvc::Service` whose handlers
//! react to fulfillment result events and publish the next step's command —
//! reserve → charge → confirm; on decline → release → cancel. Just another
//! service combining message subscribe/publish, with the saga as its model.

pub mod models;

mod handlers;
mod service;

use sourced_rust::{AggregateRepository, HashMapRepository, QueuedRepository};

pub use models::OrderFulfillmentSaga;
pub use service::service;

pub type SagaRepo = AggregateRepository<QueuedRepository<HashMapRepository>, OrderFulfillmentSaga>;
