//! Saga service handlers — coordinates the order fulfillment flow.

use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::{
    AsyncAggregateRepository, HashMapRepository, InMemoryAsyncLockManager, QueuedRepository,
};

use super::messages::*;
use crate::order::OrderFulfillmentSaga;

pub type Repo = AsyncAggregateRepository<
    QueuedRepository<HashMapRepository, InMemoryAsyncLockManager>,
    OrderFulfillmentSaga,
>;

pub mod on_inventory_reserved;
pub mod on_order_completed;
pub mod on_order_created;
pub mod on_payment_succeeded;
pub mod start;
