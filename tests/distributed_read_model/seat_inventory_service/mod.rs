mod service;

pub mod handlers;
pub mod models;

pub use models::Seat;
pub use service::service;

use sourced_rust::{
    AsyncAggregateRepository, HashMapRepository, InMemoryAsyncLockManager, QueuedRepository,
};

pub type SeatRepo =
    AsyncAggregateRepository<QueuedRepository<HashMapRepository, InMemoryAsyncLockManager>, Seat>;
