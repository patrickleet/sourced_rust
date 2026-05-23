use sourced_rust::{InMemoryReadModelStore, ReadModelError, ReadModelStore};

pub mod account_summary;

pub use account_summary::AccountSummary;

#[derive(Clone)]
pub struct AccountSummaryQueryService {
    store: InMemoryReadModelStore,
}

impl AccountSummaryQueryService {
    pub fn new(store: InMemoryReadModelStore) -> Self {
        Self { store }
    }

    pub fn get(&self, account_id: &str) -> Result<Option<AccountSummary>, ReadModelError> {
        self.store
            .get_by_primary_key::<AccountSummary>(account_id)
            .map(|summary| summary.map(|view| view.data))
    }
}
