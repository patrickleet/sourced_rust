use crate::entity::Entity;
use crate::outbox::OutboxMessage;
use crate::read_model::ReadModelWritePlan;
use crate::snapshot::SnapshotRecord;

use super::inbox::InboxReceipt;

/// A snapshot write staged as part of a transactional commit.
#[derive(Clone, Debug)]
pub enum SnapshotWrite {
    Save(SnapshotRecord),
}

/// A structured set of writes that must commit under one transaction boundary.
pub struct CommitBatch<'a> {
    pub entities: Vec<&'a mut Entity>,
    pub outbox_messages: Vec<OutboxMessage>,
    pub read_model_plans: Vec<ReadModelWritePlan>,
    pub snapshots: Vec<SnapshotWrite>,
    /// Consumer inbox receipts to record in the same transaction (the optional
    /// effectively-once effect fence). Empty for the default idempotent path.
    pub inbox_receipts: Vec<InboxReceipt>,
}

impl<'a> CommitBatch<'a> {
    pub fn new(entities: Vec<&'a mut Entity>) -> Self {
        Self {
            entities,
            outbox_messages: Vec::new(),
            read_model_plans: Vec::new(),
            snapshots: Vec::new(),
            inbox_receipts: Vec::new(),
        }
    }

    pub fn empty() -> Self {
        Self::new(Vec::new())
    }
}
