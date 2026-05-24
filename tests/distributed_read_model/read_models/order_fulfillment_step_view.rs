use serde::{Deserialize, Serialize};
use sourced_rust::ReadModel;

/// One row per saga step, projected from `fulfillment.*` events. Composite
/// primary key `[order_id, step]`; `order_id` is a delegated foreign key from
/// the order. This is the saga's audit trail as a `has_many` child of `OrderView`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "order_fulfillment_steps", primary_key = ["order_id", "step"])]
pub struct OrderFulfillmentStepView {
    #[readmodel(foreign_key = "orders.order_id", delegated_from = "OrderView.order_id")]
    pub order_id: String,
    pub step: String,
    pub detail: String,
}
