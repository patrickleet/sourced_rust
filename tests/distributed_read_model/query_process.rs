use sourced_rust::{InMemoryReadModelStore, ReadModelError, ReadModelStore};

use crate::models::readmodels::account_summary::AccountSummary;

#[derive(Clone)]
pub struct AccountSummaryQueryProcess {
    store: InMemoryReadModelStore,
}

impl AccountSummaryQueryProcess {
    pub fn new(store: InMemoryReadModelStore) -> Self {
        Self { store }
    }

    pub fn get(&self, account_id: &str) -> Result<Option<AccountSummary>, ReadModelError> {
        self.store
            .get_by_primary_key::<AccountSummary>(account_id)
            .map(|summary| summary.map(|view| view.data))
    }
}
