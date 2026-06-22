//! Saga service handlers — coordinates the order fulfillment flow.

use distributed::microsvc::{Context, HandlerError};
use distributed::{AggregateRepository, HashMapRepository, InMemoryLockManager, QueuedRepository};
use serde_json::{json, Value};

use super::messages::*;
use crate::order::OrderFulfillmentSaga;

pub type Repo = AggregateRepository<
    QueuedRepository<HashMapRepository, InMemoryLockManager>,
    OrderFulfillmentSaga,
>;

pub mod on_inventory_reserved;
pub mod on_order_completed;
pub mod on_order_created;
pub mod on_payment_succeeded;
pub mod start;
