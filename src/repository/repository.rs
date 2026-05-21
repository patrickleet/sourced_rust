use super::error::RepositoryError;
use super::gettable::{GetMany, GetOne, Gettable};
use crate::entity::Entity;

/// Get one or more aggregate event streams by ID.
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

/// Scan aggregate event streams and return entities matching a Rust predicate.
///
/// This is an in-memory scan contract: implementations may need to hydrate
/// streams before applying the predicate. Production query workloads should
/// normally use read models or explicit indexed query APIs.
pub trait Find {
    fn find<F>(&self, predicate: F) -> Result<Vec<Entity>, RepositoryError>
    where
        F: Fn(&Entity) -> bool;
}

/// Scan aggregate event streams and return the first entity matching a Rust predicate.
pub trait FindOne {
    fn find_one<F>(&self, predicate: F) -> Result<Option<Entity>, RepositoryError>
    where
        F: Fn(&Entity) -> bool;
}

/// Scan aggregate event streams and check if any entity matches a Rust predicate.
pub trait Exists {
    fn exists<F>(&self, predicate: F) -> Result<bool, RepositoryError>
    where
        F: Fn(&Entity) -> bool;
}

/// Scan aggregate event streams and count entities matching a Rust predicate.
pub trait Count {
    fn count<F>(&self, predicate: F) -> Result<usize, RepositoryError>
    where
        F: Fn(&Entity) -> bool;
}

use crate::entity::Committable;

/// Append new aggregate event records for one or more entities.
pub trait Commit {
    fn commit<C: Committable + ?Sized>(&self, committable: &mut C) -> Result<(), RepositoryError>;
}

/// Full repository trait combining all capabilities.
pub trait Repository: Get + Find + FindOne + Exists + Count + Commit {}

// Blanket implementation: anything implementing all traits is a Repository
impl<T> Repository for T where T: Get + Find + FindOne + Exists + Count + Commit {}
