use crate::repository::RepositoryError;

/// A stored snapshot record: aggregate ID, version at time of snapshot, and serialized data.
#[derive(Clone, Debug)]
pub struct SnapshotRecord {
    pub aggregate_id: String,
    pub version: u64,
    pub data: Vec<u8>,
}

/// Trait for snapshot persistence. One snapshot per aggregate ID (latest wins).
pub trait SnapshotStore: Send + Sync {
    /// Load the latest snapshot for the given aggregate ID.
    fn get_snapshot(&self, id: &str) -> Result<Option<SnapshotRecord>, RepositoryError>;

    /// Save (or overwrite) the snapshot for the given aggregate ID.
    fn save_snapshot(&self, record: SnapshotRecord) -> Result<(), RepositoryError>;

    /// Delete the snapshot for the given aggregate ID. Returns true if one existed.
    fn delete_snapshot(&self, id: &str) -> Result<bool, RepositoryError>;
}
