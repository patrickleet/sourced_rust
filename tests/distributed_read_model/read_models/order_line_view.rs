use serde::{Deserialize, Serialize};
use sourced_rust::ReadModel;

use super::ProductView;

/// One order line. Composite primary key `[order_id, sku]`; `order_id` is a
/// delegated foreign key filled from the parent order, and `product` is a
/// `belongs_to` include resolved against the catalog projection.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "order_lines", primary_key = ["order_id", "sku"])]
pub struct OrderLineView {
    #[readmodel(foreign_key = "orders.order_id", delegated_from = "OrderView.order_id")]
    pub order_id: String,
    pub sku: String,
    pub product_id: String,
    pub quantity: i64,
    pub line_total_cents: i64,
    #[readmodel(belongs_to = "ProductView", foreign_key = "product_id")]
    pub product: Option<ProductView>,
}
