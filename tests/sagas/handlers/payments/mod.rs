//! Payment service handlers.

use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::{AggregateRepository, HashMapRepository, QueuedRepository, SyncOutboxCommitExt};

use super::messages::*;
use crate::order::Payment;

pub type Repo = AggregateRepository<QueuedRepository<HashMapRepository>, Payment>;

pub mod process;
