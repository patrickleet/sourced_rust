//! Normalized relational read models shared by the projector and query
//! services. `OrderView` has_many `OrderLineView`; `OrderLineView` belongs_to
//! `ProductView`. A Hasura-style gateway could expose these tables directly as
//! object/array relationships.

mod order_fulfillment_step_view;
mod order_line_view;
mod order_view;
mod product_view;

pub use order_fulfillment_step_view::OrderFulfillmentStepView;
pub use order_line_view::OrderLineView;
pub use order_view::OrderView;
pub use product_view::ProductView;

use sourced_rust::{InMemoryReadModelStore, ReadModelError, RowKey, RowValue};

/// Register every relational schema this example reads or projects.
pub fn register_schemas(store: &InMemoryReadModelStore) -> Result<(), ReadModelError> {
    store.register_schema::<ProductView>()?;
    store.register_schema::<OrderView>()?;
    store.register_schema::<OrderLineView>()?;
    store.register_schema::<OrderFulfillmentStepView>()?;
    Ok(())
}

pub fn order_key(order_id: &str) -> RowKey {
    RowKey::new([("order_id", RowValue::String(order_id.into()))])
}

pub fn order_line_key(order_id: &str, sku: &str) -> RowKey {
    RowKey::new([
        ("order_id", RowValue::String(order_id.into())),
        ("sku", RowValue::String(sku.into())),
    ])
}
