//! Read-only query service. It owns no aggregate repository; it reads the
//! projected relational tables through primary-key loads plus explicit
//! relationship includes.

use sourced_rust::{InMemoryReadModelStore, ReadModelError, ReadModelWorkspaceExt};

use crate::read_models::{checkout_key, seat_key, CheckoutView, SeatView};

#[derive(Clone)]
pub struct CheckoutQueryService {
    store: InMemoryReadModelStore,
}

impl CheckoutQueryService {
    pub fn new(store: InMemoryReadModelStore) -> Self {
        Self { store }
    }

    /// Load the checkout screen with its audit steps and current seat row.
    pub fn checkout_screen(
        &self,
        checkout_id: &str,
    ) -> Result<Option<CheckoutView>, ReadModelError> {
        let mut session = self.store.workspace();
        Ok(session
            .load::<CheckoutView>(checkout_key(checkout_id))
            .include("steps")
            .include("seat")
            .one()?
            .map(|view| view.data))
    }

    pub fn seat(&self, seat_id: &str) -> Result<Option<SeatView>, ReadModelError> {
        let mut session = self.store.workspace();
        Ok(session
            .load::<SeatView>(seat_key(seat_id))
            .one()?
            .map(|view| view.data))
    }
}
