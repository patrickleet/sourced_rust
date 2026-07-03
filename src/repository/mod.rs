mod error;
mod identity;
mod inbox;
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
#[cfg(any(feature = "postgres", feature = "sqlite"))]
pub(crate) use validation::validate_supported_event_codec;
pub(crate) use validation::{validate_commit_batch, validate_snapshot_identity};
