mod service;

pub mod handlers;
pub mod models;

pub use models::Seat;
pub use service::service;

use distributed::{
    AggregateRepository, HashMapRepository, InMemoryAsyncLockManager, QueuedRepository,
};

pub type SeatRepo =
    AggregateRepository<QueuedRepository<HashMapRepository, InMemoryAsyncLockManager>, Seat>;
