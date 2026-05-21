//! ReadModelStore - Abstract CRUD storage for read models.

use super::{ReadModel, ReadModelError, Versioned};

/// Abstract CRUD storage for read models.
///
/// Methods that would collide with Repository traits (`Get`, `Scan`, and the
/// historical `Find` aliases) use a `_model` suffix. Non-colliding methods use
/// clean names.
/// The `ReadModelRepository` wrapper provides clean short names for all methods.
///
/// Predicate-based `find` methods are scans over this read-model store. Durable
/// stores can expose additional typed or indexed query APIs for production
/// workloads without making aggregate repositories translate arbitrary Rust
/// predicates into SQL.
pub trait ReadModelStore: Send + Sync {
    /// Get a read model by ID. Returns None if not found.
    fn get_model<M: ReadModel>(&self, id: &str) -> Result<Option<Versioned<M>>, ReadModelError>;

    /// Upsert a read model (insert or update, no version check).
    fn upsert<M: ReadModel>(&self, model: &M) -> Result<Versioned<M>, ReadModelError>;

    /// Insert a new read model. Fails if it already exists.
    fn insert<M: ReadModel>(&self, model: &M) -> Result<Versioned<M>, ReadModelError>;

    /// Update an existing read model with optimistic concurrency control.
    fn update<M: ReadModel>(
        &self,
        model: &M,
        expected_version: u64,
    ) -> Result<Versioned<M>, ReadModelError>;

    /// Delete a read model by ID. Returns true if it existed.
    fn delete<M: ReadModel>(&self, id: &str) -> Result<bool, ReadModelError>;

    /// Scan read models matching a predicate.
    fn find_models<M: ReadModel>(
        &self,
        predicate: &dyn Fn(&M) -> bool,
    ) -> Result<Vec<Versioned<M>>, ReadModelError>;

    /// Scan read models and return the first model matching a predicate.
    fn find_one_model<M: ReadModel>(
        &self,
        predicate: &dyn Fn(&M) -> bool,
    ) -> Result<Option<Versioned<M>>, ReadModelError>;

    /// Save pre-serialized read model bytes by key. Used internally by CommitBuilder
    /// for type-erased transactional batch writes.
    fn upsert_raw(&self, key: &str, bytes: Vec<u8>) -> Result<(), ReadModelError>;
}
