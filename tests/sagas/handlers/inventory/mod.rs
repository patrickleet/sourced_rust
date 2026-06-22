//! Inventory service handlers.

use distributed::microsvc::{Context, HandlerError};
use distributed::{AggregateRepository, HashMapRepository, InMemoryLockManager, QueuedRepository};
use serde_json::{json, Value};

use super::messages::*;
use crate::order::Inventory;

pub type Repo =
    AggregateRepository<QueuedRepository<HashMapRepository, InMemoryLockManager>, Inventory>;

pub mod init;
pub mod reserve;
