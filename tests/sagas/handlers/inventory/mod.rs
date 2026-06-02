//! Inventory service handlers.

use distributed::microsvc::{Context, HandlerError};
use distributed::{
    AggregateRepository, HashMapRepository, InMemoryAsyncLockManager, QueuedRepository,
};
use serde_json::{json, Value};

use super::messages::*;
use crate::order::Inventory;

pub type Repo =
    AggregateRepository<QueuedRepository<HashMapRepository, InMemoryAsyncLockManager>, Inventory>;

pub mod init;
pub mod reserve;
