use distributed::{AggregateRepository, InMemoryRepository, InMemoryLockManager, QueuedRepository};

use crate::models::counter::Counter;

pub type Repo =
    AggregateRepository<QueuedRepository<InMemoryRepository, InMemoryLockManager>, Counter>;

pub mod counter_create;
pub mod counter_increment;
pub mod whoami;
