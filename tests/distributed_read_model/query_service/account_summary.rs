use serde::{Deserialize, Serialize};
use sourced_rust::ReadModel;

#[derive(Clone, Debug, Deserialize, Serialize, ReadModel)]
#[collection("account_summaries")]
pub struct AccountSummary {
    #[id]
    pub account_id: String,
    pub owner: Option<String>,
    pub balance_cents: i64,
    pub deposit_count: u32,
    projected_event_ids: Vec<String>,
}

impl AccountSummary {
    pub fn empty(account_id: &str) -> Self {
        Self {
            account_id: account_id.to_string(),
            owner: None,
            balance_cents: 0,
            deposit_count: 0,
            projected_event_ids: Vec::new(),
        }
    }
}
