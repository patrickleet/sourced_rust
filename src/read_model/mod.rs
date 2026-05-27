//! Read Models - storage-backed projections and read-optimized views.
//!
//! Relational models stage explicit row mutations:
//!
//! ```ignore
//! use sourced_rust::{ReadModelWritePlanBuilder, ReadModelWritePlanCommitExt};
//!
//! let mut read_models = ReadModelWritePlanBuilder::new();
//! read_models.upsert(&player)?;
//! read_models.upsert_related(&player, "weapons", &weapon)?;
//! repo.read_models(read_models).commit(&mut aggregate)?;
//! ```
//!
//! Async persistent repositories expose the same staging shape through
//! `AsyncReadModelWritePlanCommitExt`, returning a future from `commit`.
//!
//! Distributed projectors can commit a write plan directly against a read-model
//! adapter and mark messages processed in the same adapter transaction:
//!
//! ```ignore
//! let mut read_models = ReadModelWritePlanBuilder::new();
//! read_models.upsert(&view)?.mark_processed("projection", event_id);
//! let outcome = read_models.commit(&read_store)?;
//! ```

pub(crate) mod in_memory;
mod metadata;
mod schema;
mod session;

use serde::{de::DeserializeOwned, Serialize};
use std::fmt;

/// Trait implemented by the derive macro for read-model identity metadata.
pub trait ReadModel: Serialize + DeserializeOwned + Clone + Send + Sync {
    /// The declared storage name for this read model type.
    const COLLECTION: &'static str;

    /// Returns the unique identifier for this read model instance.
    fn id(&self) -> &str;
}

/// A versioned wrapper around read model data for optimistic concurrency control.
#[derive(Debug, Clone, PartialEq)]
pub struct Versioned<T> {
    pub data: T,
    pub version: u64,
}

/// Error type for read model store operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadModelError {
    /// Optimistic concurrency conflict.
    ConcurrencyConflict {
        collection: String,
        id: String,
        expected: u64,
        actual: u64,
    },
    /// Serialization/deserialization error.
    Serde(String),
    /// Storage-level error.
    Storage(String),
    /// Read model not found.
    NotFound { collection: String, id: String },
    /// Lock error.
    Lock(crate::lock::LockError),
    /// Relational read-model metadata error.
    Metadata(String),
}

impl fmt::Display for ReadModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReadModelError::ConcurrencyConflict {
                collection,
                id,
                expected,
                actual,
            } => write!(
                f,
                "concurrency conflict on {}:{} (expected version {}, actual {})",
                collection, id, expected, actual
            ),
            ReadModelError::Serde(msg) => write!(f, "read model serialization error: {}", msg),
            ReadModelError::Storage(msg) => write!(f, "read model storage error: {}", msg),
            ReadModelError::NotFound { collection, id } => {
                write!(f, "read model not found: {}:{}", collection, id)
            }
            ReadModelError::Lock(err) => write!(f, "read model lock error: {}", err),
            ReadModelError::Metadata(msg) => write!(f, "read model metadata error: {}", msg),
        }
    }
}

impl std::error::Error for ReadModelError {}

impl From<crate::lock::LockError> for ReadModelError {
    fn from(err: crate::lock::LockError) -> Self {
        ReadModelError::Lock(err)
    }
}

pub use in_memory::InMemoryReadModelStore;
pub use metadata::{
    ColumnDef, ColumnType, ForeignKey, IndexDef, PrimaryKey, ReadModelSchema, RelationalReadModel,
    RelationalReadModelIncludes, RelationshipDef, RelationshipKind, RowKey, RowValue, RowValues,
    DEFAULT_READ_MODEL_VERSION_COLUMN,
};
pub use schema::{
    ReadModelMigrationArtifact, ReadModelSchemaAdapter, ReadModelSchemaAdapterCapabilities,
    ReadModelSchemaBootstrap, ReadModelSchemaIssue, ReadModelSchemaIssueKind,
    ReadModelSchemaRegistry, ReadModelSchemaVerification,
};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
pub(crate) use session::{column_name_for, key_fingerprint, validate_key, validate_row_values};
pub use session::{
    DeleteRowMutation, ExpectedVersion, PatchMode, PatchRowMutation, ProcessedMessageMark,
    ReadModelAdapterCapabilities, ReadModelCommitOutcome, ReadModelIncludeRows, ReadModelLoadGraph,
    ReadModelLoadRequest, ReadModelMutation, ReadModelQueryCapabilities, ReadModelWorkspace,
    ReadModelWorkspaceExt, ReadModelWritePlan, ReadModelWritePlanBuilder, ReadModelWritePlanStore,
    RelationalReadModelQueryStore, RowMutation, RowPatch, RowWriteMode,
};
