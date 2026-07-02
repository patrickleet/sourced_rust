//! Shared error type for table stores and the read models built on them.

use std::fmt;

/// Error type for table-store and read-model operations.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TableStoreError {
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
    /// Row not found.
    NotFound { collection: String, id: String },
    /// Lock error.
    Lock(crate::lock::LockError),
    /// Schema/metadata error.
    Metadata(String),
}

impl fmt::Display for TableStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TableStoreError::ConcurrencyConflict {
                collection,
                id,
                expected,
                actual,
            } => write!(
                f,
                "concurrency conflict on {}:{} (expected version {}, actual {})",
                collection, id, expected, actual
            ),
            TableStoreError::Serde(msg) => write!(f, "table store serialization error: {}", msg),
            TableStoreError::Storage(msg) => write!(f, "table storage error: {}", msg),
            TableStoreError::NotFound { collection, id } => {
                write!(f, "table row not found: {}:{}", collection, id)
            }
            TableStoreError::Lock(err) => write!(f, "table store lock error: {}", err),
            TableStoreError::Metadata(msg) => write!(f, "table metadata error: {}", msg),
        }
    }
}

impl std::error::Error for TableStoreError {}

impl From<crate::lock::LockError> for TableStoreError {
    fn from(err: crate::lock::LockError) -> Self {
        TableStoreError::Lock(err)
    }
}
