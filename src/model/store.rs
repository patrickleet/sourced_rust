//! ModelStore - Abstract CRUD storage for models.

use super::{Model, ModelError, Versioned};

/// Abstract CRUD storage for models.
pub trait ModelStore: Send + Sync {
    /// Get a model by ID. Returns None if not found.
    fn get_model<M: Model>(&self, id: &str) -> Result<Option<Versioned<M>>, ModelError>;

    /// Upsert a model (insert or update, no version check).
    fn save_model<M: Model>(&self, model: &M) -> Result<Versioned<M>, ModelError>;

    /// Insert a new model. Fails if it already exists.
    fn insert_model<M: Model>(&self, model: &M) -> Result<Versioned<M>, ModelError>;

    /// Update an existing model with optimistic concurrency control.
    fn update_model<M: Model>(
        &self,
        model: &M,
        expected_version: u64,
    ) -> Result<Versioned<M>, ModelError>;

    /// Delete a model by ID. Returns true if it existed.
    fn delete_model<M: Model>(&self, id: &str) -> Result<bool, ModelError>;

    /// Find models matching a predicate.
    fn find_models<M: Model>(
        &self,
        predicate: &dyn Fn(&M) -> bool,
    ) -> Result<Vec<Versioned<M>>, ModelError>;

    /// Save pre-serialized model bytes by key. Used internally by CommitBuilder
    /// for type-erased atomic writes.
    fn save_model_raw(&self, key: &str, bytes: Vec<u8>) -> Result<(), ModelError>;
}
