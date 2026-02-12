use sourced_rust::{AggregateRepository, HashMapRepository, QueuedRepository};

use crate::models::counter::Counter;

pub type Repo = AggregateRepository<QueuedRepository<HashMapRepository>, Counter>;

pub mod counter_create;
pub mod counter_increment;
pub mod whoami;
