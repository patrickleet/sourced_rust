//! Order service handlers.

use distributed::microsvc::{Context, HandlerError};
use distributed::{
    AsyncAggregateRepository, HashMapRepository, InMemoryAsyncLockManager, QueuedRepository,
};
use serde_json::{json, Value};

use super::messages::*;
use crate::order::Order;

pub type Repo =
    AsyncAggregateRepository<QueuedRepository<HashMapRepository, InMemoryAsyncLockManager>, Order>;

pub mod complete;
pub mod create;
