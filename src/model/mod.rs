//! Models - Storage-backed data for projections and non-ES entities.
//!
//! Models provide a simple CRUD abstraction for storing typed data,
//! used for projections (read models) and standalone non-ES entities.
//!
//! ## Example
//!
//! ```ignore
//! use sourced_rust::{Model, InMemoryModelStore, ModelsExt, Versioned};
//!
//! #[derive(Serialize, Deserialize, Clone)]
//! struct GameView {
//!     pub id: String,
//!     pub score: u32,
//! }
//!
//! impl Model for GameView {
//!     const COLLECTION: &'static str = "game_views";
//!     fn id(&self) -> &str { &self.id }
//! }
//!
//! let store = InMemoryModelStore::new();
//! store.models::<GameView>().save(&view)?;
//! let loaded = store.models::<GameView>().get("game-1")?;
//! ```

mod in_memory;
mod model_repository;
mod store;

use serde::{de::DeserializeOwned, Serialize};
use std::fmt;

/// Trait for types that can be stored as models.
pub trait Model: Serialize + DeserializeOwned + Clone + Send + Sync {
    /// The collection name for this model type (e.g., "game_views", "user_profiles").
    /// Maps to a table in SQL, a collection in MongoDB, a key prefix in KV stores, etc.
    const COLLECTION: &'static str;

    /// Returns the unique identifier for this model instance.
    fn id(&self) -> &str;
}

/// A versioned wrapper around model data for optimistic concurrency control.
#[derive(Debug, Clone)]
pub struct Versioned<T> {
    pub data: T,
    pub version: u64,
}

/// Error type for model store operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelError {
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
    /// Model not found.
    NotFound { collection: String, id: String },
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModelError::ConcurrencyConflict {
                collection,
                id,
                expected,
                actual,
            } => write!(
                f,
                "concurrency conflict on {}:{} (expected version {}, actual {})",
                collection, id, expected, actual
            ),
            ModelError::Serde(msg) => write!(f, "model serialization error: {}", msg),
            ModelError::Storage(msg) => write!(f, "model storage error: {}", msg),
            ModelError::NotFound { collection, id } => {
                write!(f, "model not found: {}:{}", collection, id)
            }
        }
    }
}

impl std::error::Error for ModelError {}

pub use in_memory::InMemoryModelStore;
pub use model_repository::{ModelRepository, ModelsExt};
pub use store::ModelStore;
