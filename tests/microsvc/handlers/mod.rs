use distributed::{
    AsyncAggregateRepository, HashMapRepository, InMemoryAsyncLockManager, QueuedRepository,
};

use crate::models::counter::Counter;

pub type Repo = AsyncAggregateRepository<
    QueuedRepository<HashMapRepository, InMemoryAsyncLockManager>,
    Counter,
>;

pub mod counter_create;
pub mod counter_increment;
pub mod whoami;
