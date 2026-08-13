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
    /// Backend storage failure with its retry classification preserved.
    ///
    /// SQL adapters use this instead of flattening a driver error into
    /// [`Storage`](Self::Storage), because projector runners must distinguish a
    /// transient busy/deadlock/connection failure from a deterministic fault
    /// before deciding whether terminal failure evidence may be recorded.
    BackendStorage {
        operation: String,
        retryable: bool,
        message: String,
    },
    /// A raw/legacy write targeted a table registered to the causal projection
    /// protocol and therefore cannot mint the required revision evidence.
    CausalWriteRequired { table: String },
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
            TableStoreError::BackendStorage {
                operation,
                retryable,
                message,
            } => {
                let class = if *retryable { "retryable" } else { "permanent" };
                write!(
                    f,
                    "table backend storage error ({class}) during {operation}: {message}"
                )
            }
            TableStoreError::CausalWriteRequired { table } => write!(
                f,
                "table `{table}` is causal-owned and requires the projection commit path"
            ),
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
