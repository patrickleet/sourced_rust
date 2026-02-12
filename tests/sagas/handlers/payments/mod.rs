//! Payment service handlers.

use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::{AggregateRepository, HashMapRepository, OutboxCommitExt, QueuedRepository};

use crate::order::Payment;
use super::messages::*;

pub type Repo = AggregateRepository<QueuedRepository<HashMapRepository>, Payment>;

pub mod process;
