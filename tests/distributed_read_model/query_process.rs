use sourced_rust::{HashMapRepository, ReadModelError, ReadModelsExt};

use crate::models::readmodels::account_summary::AccountSummary;

#[derive(Clone)]
pub struct AccountSummaryQueryProcess {
    store: HashMapRepository,
}

impl AccountSummaryQueryProcess {
    pub fn new(store: HashMapRepository) -> Self {
        Self { store }
    }

    pub fn get(&self, account_id: &str) -> Result<Option<AccountSummary>, ReadModelError> {
        self.store
            .read_models::<AccountSummary>()
            .get(account_id)
            .map(|summary| summary.map(|view| view.data))
    }
}
