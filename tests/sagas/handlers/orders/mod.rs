//! Order service handlers.

use distributed::microsvc::{Context, HandlerError};
use distributed::{
    AggregateRepository, HashMapRepository, InMemoryAsyncLockManager, QueuedRepository,
};
use serde_json::{json, Value};

use super::messages::*;
use crate::order::Order;

pub type Repo =
    AggregateRepository<QueuedRepository<HashMapRepository, InMemoryAsyncLockManager>, Order>;

pub mod complete;
pub mod create;
