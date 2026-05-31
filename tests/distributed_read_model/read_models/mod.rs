//! Normalized relational read models shared by the projector and query
//! services. `CheckoutView` has_many `CheckoutStepView` and belongs_to
//! `SeatView`, mirroring a query gateway over projected tables.

mod checkout_step_view;
mod checkout_view;
mod seat_view;

pub use checkout_step_view::CheckoutStepView;
pub use checkout_view::CheckoutView;
pub use seat_view::SeatView;

#[cfg(any(feature = "postgres", feature = "sqlite"))]
use distributed::TableSchemaRegistry;
use distributed::{InMemoryReadModelStore, ReadModelError, RowKey, RowValue};

pub fn register_schemas(store: &InMemoryReadModelStore) -> Result<(), ReadModelError> {
    store.register_schema::<SeatView>()?;
    store.register_schema::<CheckoutView>()?;
    store.register_schema::<CheckoutStepView>()?;
    Ok(())
}

#[cfg(any(feature = "postgres", feature = "sqlite"))]
pub fn table_schema_registry() -> Result<TableSchemaRegistry, ReadModelError> {
    let mut registry = TableSchemaRegistry::new();
    registry.register::<SeatView>()?;
    registry.register::<CheckoutView>()?;
    registry.register::<CheckoutStepView>()?;
    Ok(registry)
}

pub fn checkout_key(checkout_id: &str) -> RowKey {
    RowKey::new([("checkout_id", RowValue::String(checkout_id.into()))])
}

pub fn seat_key(seat_id: &str) -> RowKey {
    RowKey::new([("seat_id", RowValue::String(seat_id.into()))])
}
