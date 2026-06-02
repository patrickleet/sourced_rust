use distributed::{
    AggregateRepository, HashMapRepository, InMemoryAsyncLockManager, QueuedRepository,
};

use crate::models::counter::Counter;

pub type Repo =
    AggregateRepository<QueuedRepository<HashMapRepository, InMemoryAsyncLockManager>, Counter>;

pub mod counter_create;
pub mod counter_increment;
pub mod whoami;
