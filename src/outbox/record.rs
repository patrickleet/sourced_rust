use std::time::SystemTime;

use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub enum OutboxStatus {
    Pending,
    InFlight,
    Published,
    Failed,
}

/// Durable domain event for external publication via the outbox pattern.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct OutboxRecord {
    pub id: u64,
    pub aggregate_id: String,
    pub aggregate_version: u64,
    pub event_type: String,
    pub payload: String,
    pub occurred_at: SystemTime,
    pub status: OutboxStatus,
    pub attempts: u32,
    pub locked_by: Option<String>,
    pub locked_until: Option<SystemTime>,
    pub published_at: Option<SystemTime>,
    pub failed_at: Option<SystemTime>,
    pub last_error: Option<String>,
}
