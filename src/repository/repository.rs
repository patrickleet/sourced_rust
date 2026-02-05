use crate::entity::Entity;
use super::error::RepositoryError;
use super::gettable::{GetMany, GetOne, Gettable};

/// Get one or more entities by ID(s).
pub trait Get: GetOne + GetMany {
    fn get<G: Gettable>(&self, gettable: G) -> Result<G::Output, RepositoryError>
    where
        Self: Sized,
    {
        gettable.get_from(self)
    }
}

// Blanket implementation: anything implementing GetOne + GetMany is Get
impl<T: GetOne + GetMany> Get for T {}

/// Find all entities matching a predicate.
pub trait Find {
    fn find<F>(&self, predicate: F) -> Result<Vec<Entity>, RepositoryError>
    where
        F: Fn(&Entity) -> bool;
}

/// Find the first entity matching a predicate.
pub trait FindOne {
    fn find_one<F>(&self, predicate: F) -> Result<Option<Entity>, RepositoryError>
    where
        F: Fn(&Entity) -> bool;
}

/// Check if any entity matches a predicate.
pub trait Exists {
    fn exists<F>(&self, predicate: F) -> Result<bool, RepositoryError>
    where
        F: Fn(&Entity) -> bool;
}

/// Count entities matching a predicate.
pub trait Count {
    fn count<F>(&self, predicate: F) -> Result<usize, RepositoryError>
    where
        F: Fn(&Entity) -> bool;
}

use crate::entity::Committable;

/// Commit one or more entities.
pub trait Commit {
    fn commit<C: Committable + ?Sized>(&self, committable: &mut C) -> Result<(), RepositoryError>;
}

/// Full repository trait combining all capabilities.
pub trait Repository: Get + Find + FindOne + Exists + Count + Commit {}

// Blanket implementation: anything implementing all traits is a Repository
impl<T> Repository for T where T: Get + Find + FindOne + Exists + Count + Commit {}

