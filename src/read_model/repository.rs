//! ReadModelRepository - Typed accessor for read model CRUD operations.

use std::marker::PhantomData;

use super::{ReadModel, ReadModelError, ReadModelStore, Versioned};

/// Typed repository wrapper for accessing read models of a specific type.
///
/// Provides clean short method names by delegating to `ReadModelStore` trait methods.
pub struct ReadModelRepository<'a, S, M> {
    store: &'a S,
    _marker: PhantomData<M>,
}

impl<'a, S: ReadModelStore, M: ReadModel> ReadModelRepository<'a, S, M> {
    pub fn new(store: &'a S) -> Self {
        Self {
            store,
            _marker: PhantomData,
        }
    }

    /// Get a read model by ID.
    pub fn get(&self, id: &str) -> Result<Option<Versioned<M>>, ReadModelError> {
        self.store.get_model(id)
    }

    /// Upsert a read model (insert or update, no version check).
    pub fn upsert(&self, model: &M) -> Result<Versioned<M>, ReadModelError> {
        self.store.upsert(model)
    }

    /// Insert a new read model. Fails if it already exists.
    pub fn insert(&self, model: &M) -> Result<Versioned<M>, ReadModelError> {
        self.store.insert(model)
    }

    /// Update an existing read model with optimistic concurrency.
    pub fn update(&self, model: &M, expected_version: u64) -> Result<Versioned<M>, ReadModelError> {
        self.store.update(model, expected_version)
    }

    /// Delete a read model by ID. Returns true if it existed.
    pub fn delete(&self, id: &str) -> Result<bool, ReadModelError> {
        self.store.delete::<M>(id)
    }

    /// Find read models matching a predicate.
    pub fn find(&self, predicate: &dyn Fn(&M) -> bool) -> Result<Vec<Versioned<M>>, ReadModelError> {
        self.store.find_models(predicate)
    }

    /// Find the first read model matching a predicate.
    pub fn find_one(&self, predicate: &dyn Fn(&M) -> bool) -> Result<Option<Versioned<M>>, ReadModelError> {
        self.store.find_one_model(predicate)
    }
}

/// Extension trait for typed read model access on any ReadModelStore.
pub trait ReadModelsExt: ReadModelStore + Sized {
    /// Get a typed read model repository.
    fn read_models<M: ReadModel>(&self) -> ReadModelRepository<'_, Self, M> {
        ReadModelRepository::new(self)
    }
}

impl<S: ReadModelStore> ReadModelsExt for S {}
