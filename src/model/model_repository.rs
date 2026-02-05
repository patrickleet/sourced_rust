//! ModelRepository - Typed accessor for model CRUD operations.

use std::marker::PhantomData;

use super::{Model, ModelError, ModelStore, Versioned};

/// Typed repository wrapper for accessing models of a specific type.
pub struct ModelRepository<'a, S, M> {
    store: &'a S,
    _marker: PhantomData<M>,
}

impl<'a, S: ModelStore, M: Model> ModelRepository<'a, S, M> {
    pub fn new(store: &'a S) -> Self {
        Self {
            store,
            _marker: PhantomData,
        }
    }

    /// Get a model by ID.
    pub fn get(&self, id: &str) -> Result<Option<Versioned<M>>, ModelError> {
        self.store.get_model(id)
    }

    /// Upsert a model (insert or update, no version check).
    pub fn save(&self, model: &M) -> Result<Versioned<M>, ModelError> {
        self.store.save_model(model)
    }

    /// Insert a new model. Fails if it already exists.
    pub fn insert(&self, model: &M) -> Result<Versioned<M>, ModelError> {
        self.store.insert_model(model)
    }

    /// Update an existing model with optimistic concurrency.
    pub fn update(&self, model: &M, expected_version: u64) -> Result<Versioned<M>, ModelError> {
        self.store.update_model(model, expected_version)
    }

    /// Delete a model by ID. Returns true if it existed.
    pub fn delete(&self, id: &str) -> Result<bool, ModelError> {
        self.store.delete_model::<M>(id)
    }

    /// Find models matching a predicate.
    pub fn find(&self, predicate: &dyn Fn(&M) -> bool) -> Result<Vec<Versioned<M>>, ModelError> {
        self.store.find_models(predicate)
    }
}

/// Extension trait for typed model access on any ModelStore.
pub trait ModelsExt: ModelStore + Sized {
    /// Get a typed model repository.
    fn models<M: Model>(&self) -> ModelRepository<'_, Self, M> {
        ModelRepository::new(self)
    }
}

impl<S: ModelStore> ModelsExt for S {}
