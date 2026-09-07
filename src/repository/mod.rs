mod error;
mod identity;
mod inbox;
pub(crate) mod sql;
pub(crate) mod sqlite_codec;
mod traits;
mod validation;

pub use error::RepositoryError;
pub use identity::StreamIdentity;
pub use inbox::{InboxOutcome, InboxReceipt};
pub use traits::{
    CommitBatch, GetStream, InboxStore, PreparedEventAppend, ReadModelWritePlanStore,
    RelationalReadModelQueryStore, Repository, SnapshotStore, SnapshotWrite, StreamWrite,
    TransactionalCommit,
};
pub(crate) use validation::{validate_commit_batch, validate_snapshot_identity};
