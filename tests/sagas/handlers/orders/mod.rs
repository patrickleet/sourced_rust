//! Order service handlers.

use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::{
    AsyncAggregateRepository, HashMapRepository, InMemoryAsyncLockManager, QueuedRepository,
};

use super::messages::*;
use crate::order::Order;

pub type Repo =
    AsyncAggregateRepository<QueuedRepository<HashMapRepository, InMemoryAsyncLockManager>, Order>;

pub mod complete;
pub mod create;
