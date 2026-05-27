//! Order service handlers.

use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::{AggregateRepository, HashMapRepository, QueuedRepository, SyncOutboxCommitExt};

use super::messages::*;
use crate::order::Order;

pub type Repo = AggregateRepository<QueuedRepository<HashMapRepository>, Order>;

pub mod complete;
pub mod create;
