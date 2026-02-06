//! Read Models - Storage-backed data for projections and read-optimized views.
//!
//! Read models provide a simple CRUD abstraction for storing typed data,
//! updated atomically alongside event-sourced aggregates.
//!
//! ## Example
//!
//! ```ignore
//! use sourced_rust::{ReadModel, InMemoryReadModelStore, ReadModelsExt, Versioned};
//!
//! #[derive(Serialize, Deserialize, Clone, ReadModel)]
//! #[readmodel(collection = "game_views")]
//! struct GameView {
//!     #[readmodel(id)]
//!     pub id: String,
//!     pub score: u32,
//! }
//!
//! let store = InMemoryReadModelStore::new();
//! store.read_models::<GameView>().upsert(&view)?;
//! let loaded = store.read_models::<GameView>().get("game-1")?;
//! ```

mod in_memory;
mod repository;
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
#[derive(Debug, Clone)]
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
        }
    }
}

impl std::error::Error for ReadModelError {}

pub use in_memory::InMemoryReadModelStore;
pub use repository::{ReadModelRepository, ReadModelsExt};
pub use store::ReadModelStore;
