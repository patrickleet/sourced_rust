use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sourced_rust::ReadModel;

use super::{OrderFulfillmentStepView, OrderLineView};

/// Order header row. `has_many` order lines and fulfillment steps, a
/// `total_cents` rollup, a JSONB `metadata` column, and `source_version` so the
/// projector can ignore stale snapshots under out-of-order delivery.
///
/// `lines` are owned by the order projector; `fulfillment_steps` by the
/// fulfillment projector (disjoint ownership, no version contention). Both can
/// be requested in one multi-include query.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "orders")]
pub struct OrderView {
    #[readmodel(id, column = "order_id")]
    pub order_id: String,
    pub customer: String,
    pub status: String,
    pub source_version: i64,
    pub total_cents: i64,
    #[readmodel(jsonb)]
    pub metadata: BTreeMap<String, String>,
    #[readmodel(has_many = "OrderLineView", foreign_key = "order_id")]
    pub lines: Vec<OrderLineView>,
    #[readmodel(has_many = "OrderFulfillmentStepView", foreign_key = "order_id")]
    pub fulfillment_steps: Vec<OrderFulfillmentStepView>,
}
