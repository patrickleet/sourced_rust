//! Inventory service handlers.

use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::{AggregateRepository, HashMapRepository, OutboxCommitExt, QueuedRepository};

use super::messages::*;
use crate::order::Inventory;

pub type Repo = AggregateRepository<QueuedRepository<HashMapRepository>, Inventory>;

pub mod init;
pub mod reserve;
