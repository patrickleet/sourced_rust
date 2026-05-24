//! Read Models - storage-backed projections and read-optimized views.
//!
//! The crate supports one read-model write-plan surface with two row shapes:
//!
//! - document rows through [`ReadModelStore`] and collection/id JSON payloads;
//! - normalized relational rows through [`RelationalReadModel`],
//!   [`ReadModelSession`], [`ReadModelWritePlan`], and schema metadata.
//!
//! Document views can use typed key/value CRUD:
//!
//! ```ignore
//! use sourced_rust::{InMemoryReadModelStore, ReadModel, ReadModelsExt};
//!
//! #[derive(Serialize, Deserialize, Clone, ReadModel)]
//! #[readmodel(collection = "game_views")]
//! struct GameView {
//!     #[readmodel(id)]
//!     id: String,
//!     score: u32,
//! }
//!
//! let store = InMemoryReadModelStore::new();
//! store.read_models::<GameView>().upsert(&view)?;
//! let loaded = store.read_models::<GameView>().get_by_primary_key("game-1")?;
//! ```
//!
//! Relational models stage explicit row mutations:
//!
//! ```ignore
//! use sourced_rust::{ReadModelSession, ReadModelSessionCommitExt};
//!
//! let mut read_models = ReadModelSession::new();
//! read_models.save(&player)?;
//! read_models.save_related(&player, "weapons", &weapon)?;
//! repo.read_models(read_models).commit(&mut aggregate)?;
//! ```
//!
//! Distributed projectors can commit a session directly against a read-model
//! adapter and mark messages processed in the same adapter transaction:
//!
//! ```ignore
//! let mut read_models = ReadModelSession::new();
//! read_models.document(&view)?.mark_processed("projection", event_id);
//! let outcome = read_models.commit(&read_store)?;
//! ```

pub(crate) mod in_memory;
mod metadata;
mod queued;
mod repository;
mod schema;
mod session;
mod store;

use serde::{de::DeserializeOwned, Serialize};
use std::fmt;

/// Trait for types that can be stored as read models.
pub trait ReadModel: Serialize + DeserializeOwned + Clone + Send + Sync {
    /// The collection name for this read model type (e.g., "game_views", "user_profiles").
    /// Maps to a table in SQL, a collection in MongoDB, a key prefix in KV stores, etc.
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
pub use queued::QueuedReadModelStore;
pub use repository::{ReadModelRepository, ReadModelsExt};
pub use schema::{
    ReadModelMigrationArtifact, ReadModelSchemaAdapter, ReadModelSchemaAdapterCapabilities,
    ReadModelSchemaBootstrap, ReadModelSchemaIssue, ReadModelSchemaIssueKind,
    ReadModelSchemaRegistry, ReadModelSchemaVerification,
};
pub use session::{
    DeleteRowMutation, DocumentMutation, ExpectedVersion, PatchMode, PatchRowMutation,
    ProcessedMessageMark, ReadModelAdapterCapabilities, ReadModelCommitOutcome,
    ReadModelIncludeRows, ReadModelLoadGraph, ReadModelLoadRequest, ReadModelMutation,
    ReadModelQueryCapabilities, ReadModelSession, ReadModelSessionStore,
    ReadModelSessionUnitOfWork, ReadModelUnitOfWorkExt, ReadModelWritePlan,
    RelationalReadModelQueryStore, RowMutation, RowPatch, RowWriteMode,
};
pub use store::ReadModelStore;
