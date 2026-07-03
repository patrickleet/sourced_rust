//! Order service handlers.

use distributed::microsvc::{Context, HandlerError};
use distributed::{AggregateRepository, InMemoryRepository, InMemoryLockManager, QueuedRepository};
use serde_json::{json, Value};

use super::messages::*;
use crate::order::Order;

pub type Repo =
    AggregateRepository<QueuedRepository<InMemoryRepository, InMemoryLockManager>, Order>;

pub mod complete;
pub mod create;
