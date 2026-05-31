mod service;

pub mod handlers;
pub mod models;

pub use models::Seat;
pub use service::service;

use distributed::{
    AsyncAggregateRepository, HashMapRepository, InMemoryAsyncLockManager, QueuedRepository,
};

pub type SeatRepo =
    AsyncAggregateRepository<QueuedRepository<HashMapRepository, InMemoryAsyncLockManager>, Seat>;
