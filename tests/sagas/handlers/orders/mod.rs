//! Order service handlers.

use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::{AggregateRepository, HashMapRepository, OutboxCommitExt, QueuedRepository};

use crate::order::Order;
use super::messages::*;

pub type Repo = AggregateRepository<QueuedRepository<HashMapRepository>, Order>;

pub mod complete;
pub mod create;
