//! Saga service handlers — coordinates the order fulfillment flow.

use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::{AggregateRepository, HashMapRepository, OutboxCommitExt, QueuedRepository};

use crate::order::OrderFulfillmentSaga;
use super::messages::*;

pub type Repo = AggregateRepository<QueuedRepository<HashMapRepository>, OrderFulfillmentSaga>;

pub mod on_inventory_reserved;
pub mod on_order_completed;
pub mod on_order_created;
pub mod on_payment_succeeded;
pub mod start;
