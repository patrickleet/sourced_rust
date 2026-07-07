//! Payment service handlers.

use distributed::microsvc::{Context, HandlerError};
use distributed::{AggregateRepository, InMemoryLockManager, InMemoryRepository, QueuedRepository};
use serde_json::{json, Value};

use super::messages::*;
use crate::order::Payment;

pub type Repo =
    AggregateRepository<QueuedRepository<InMemoryRepository, InMemoryLockManager>, Payment>;

pub mod process;
