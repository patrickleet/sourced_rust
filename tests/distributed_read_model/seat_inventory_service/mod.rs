mod service;

pub mod handlers;
pub mod models;

pub use models::Seat;
pub use service::service;

use distributed::{AggregateRepository, HashMapRepository, InMemoryLockManager, QueuedRepository};

pub type SeatRepo =
    AggregateRepository<QueuedRepository<HashMapRepository, InMemoryLockManager>, Seat>;
