use distributed::{AggregateRepository, HashMapRepository, InMemoryLockManager, QueuedRepository};

use crate::models::counter::Counter;

pub type Repo =
    AggregateRepository<QueuedRepository<HashMapRepository, InMemoryLockManager>, Counter>;

pub mod counter_create;
pub mod counter_increment;
pub mod whoami;
