use crate::entity::Entity;
use crate::read_model::ReadModelWritePlan;
use crate::snapshot::SnapshotRecord;

use super::RepositoryError;

/// A snapshot write staged as part of a transactional commit.
#[derive(Clone, Debug)]
pub enum SnapshotWrite {
    Save(SnapshotRecord),
}

/// A structured set of writes that must commit under one transaction boundary.
pub struct CommitBatch<'a> {
    pub entities: Vec<&'a mut Entity>,
    pub read_model_plans: Vec<ReadModelWritePlan>,
    pub snapshots: Vec<SnapshotWrite>,
}

impl<'a> CommitBatch<'a> {
    pub fn new(entities: Vec<&'a mut Entity>) -> Self {
        Self {
            entities,
            read_model_plans: Vec::new(),
            snapshots: Vec::new(),
        }
    }

    pub fn empty() -> Self {
        Self::new(Vec::new())
    }
}

/// Repository capability for writes that must commit or roll back together.
pub trait TransactionalCommit {
    fn commit_batch(&self, batch: CommitBatch<'_>) -> Result<(), RepositoryError>;
}
