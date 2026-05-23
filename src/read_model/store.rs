//! ReadModelStore - Abstract CRUD storage for read models.

use super::{ReadModel, ReadModelError, Versioned};

/// Abstract CRUD storage for read models.
///
/// Methods that would collide with repository-level APIs use a `_model` suffix.
/// Non-colliding methods use clean names.
/// The `ReadModelRepository` wrapper provides clean short names for all methods.
pub trait ReadModelStore: Send + Sync {
    /// Get a read model by ID. Returns None if not found.
    fn get_model<M: ReadModel>(&self, id: &str) -> Result<Option<Versioned<M>>, ReadModelError>;

    /// Read a model by its primary key without implying command-side ownership.
    ///
    /// This is a query/read helper. Lock-aware wrappers may override it to use
    /// their non-locking read path.
    fn get_by_primary_key<M: ReadModel>(
        &self,
        id: &str,
    ) -> Result<Option<Versioned<M>>, ReadModelError> {
        self.get_model(id)
    }

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

    /// Find read models matching a predicate.
    ///
    /// This is an in-memory/document-store helper. SQL adapters are not
    /// required to translate arbitrary Rust closures into queries.
    fn find_models<M: ReadModel>(
        &self,
        predicate: &dyn Fn(&M) -> bool,
    ) -> Result<Vec<Versioned<M>>, ReadModelError>;

    /// Find the first read model matching a predicate.
    ///
    /// This is an in-memory/document-store helper. SQL adapters are not
    /// required to translate arbitrary Rust closures into queries.
    fn find_one_model<M: ReadModel>(
        &self,
        predicate: &dyn Fn(&M) -> bool,
    ) -> Result<Option<Versioned<M>>, ReadModelError>;
}
