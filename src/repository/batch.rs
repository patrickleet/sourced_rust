use crate::entity::Entity;
use crate::snapshot::SnapshotRecord;

use super::RepositoryError;

/// A typed read-model write staged as part of a transactional commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadModelWrite {
    /// Storage key in the form `collection:id`.
    pub key: String,
    /// Serialized read model bytes.
    pub bytes: Vec<u8>,
}

impl ReadModelWrite {
    pub fn new(key: impl Into<String>, bytes: Vec<u8>) -> Self {
        Self {
            key: key.into(),
            bytes,
        }
    }
}

/// A snapshot write staged as part of a transactional commit.
#[derive(Clone, Debug)]
pub enum SnapshotWrite {
    Save(SnapshotRecord),
}

/// A structured set of writes that must commit under one transaction boundary.
pub struct CommitBatch<'a> {
    pub entities: Vec<&'a mut Entity>,
    pub read_models: Vec<ReadModelWrite>,
    pub snapshots: Vec<SnapshotWrite>,
}

impl<'a> CommitBatch<'a> {
    pub fn new(entities: Vec<&'a mut Entity>) -> Self {
        Self {
            entities,
            read_models: Vec::new(),
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
