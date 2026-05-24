mod service;

pub mod handlers;
pub mod models;

pub use models::Seat;
pub use service::service;

use sourced_rust::{AggregateRepository, HashMapRepository, QueuedRepository};

pub type SeatRepo = AggregateRepository<QueuedRepository<HashMapRepository>, Seat>;
