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
pub(crate) use validation::{
    reject_duplicate_outbox_messages, reject_duplicate_streams,
    validate_entity_id_matches_identity, validate_prepared_appends, validate_snapshot_identity,
    validate_supported_event_codec,
};
