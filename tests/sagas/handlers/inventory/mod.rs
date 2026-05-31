//! Inventory service handlers.

use distributed::microsvc::{Context, HandlerError};
use distributed::{
    AsyncAggregateRepository, HashMapRepository, InMemoryAsyncLockManager, QueuedRepository,
};
use serde_json::{json, Value};

use super::messages::*;
use crate::order::Inventory;

pub type Repo = AsyncAggregateRepository<
    QueuedRepository<HashMapRepository, InMemoryAsyncLockManager>,
    Inventory,
>;

pub mod init;
pub mod reserve;
