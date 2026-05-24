//! Order write service: owns the `Order` aggregate, which holds its line items
//! as aggregate state. Every command publishes the full order snapshot through
//! the outbox so the projector can reconcile normalized rows.

pub mod models;

mod handlers;
mod service;

use sourced_rust::{AggregateRepository, HashMapRepository, QueuedRepository};

pub use models::{
    AddLine, ChangeQuantity, Order, OrderSnapshot, PlaceOrder, RemoveLine, SubmitOrder,
};
pub use service::model_service;
#[cfg(feature = "grpc")]
pub use service::start_grpc_service;
#[cfg(feature = "http")]
pub use service::start_http_service;

pub type OrderRepo = AggregateRepository<QueuedRepository<HashMapRepository>, Order>;
