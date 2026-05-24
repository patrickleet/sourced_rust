//! Read-only query service. It owns no aggregate repository; it reads the
//! projected relational tables through primary-key loads plus explicit
//! relationship includes (the in-library equivalent of a Hasura object/array
//! relationship query).

use sourced_rust::{InMemoryReadModelStore, ReadModelError, ReadModelUnitOfWorkExt};

use crate::read_models::{order_key, order_line_key, OrderLineView, OrderView};

#[derive(Clone)]
pub struct OrderQueryService {
    store: InMemoryReadModelStore,
}

impl OrderQueryService {
    pub fn new(store: InMemoryReadModelStore) -> Self {
        Self { store }
    }

    /// Load an order with both its lines and its fulfillment steps (two includes
    /// on one root — lines owned by the order projector, steps by the fulfillment
    /// projector).
    pub fn order_with_lines_and_steps(
        &self,
        order_id: &str,
    ) -> Result<Option<OrderView>, ReadModelError> {
        let mut session = self.store.session();
        Ok(session
            .load::<OrderView>(order_key(order_id))
            .include("lines")
            .include("fulfillment_steps")
            .one()?
            .map(|view| view.data))
    }

    /// Load one line with its product (`belongs_to` include).
    pub fn line_with_product(
        &self,
        order_id: &str,
        sku: &str,
    ) -> Result<Option<OrderLineView>, ReadModelError> {
        let mut session = self.store.session();
        Ok(session
            .load::<OrderLineView>(order_line_key(order_id, sku))
            .include("product")
            .one()?
            .map(|view| view.data))
    }
}
