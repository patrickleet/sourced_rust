use super::error::RepositoryError;
use super::gettable::{GetMany, GetOne, Gettable};

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

use crate::entity::Committable;

/// Append new aggregate event records for one or more entities.
pub trait Commit {
    fn commit<C: Committable + ?Sized>(&self, committable: &mut C) -> Result<(), RepositoryError>;
}

/// Repository trait for types that implement both read-by-ID and commit APIs.
pub trait Repository: Get + Commit {}

// Blanket implementation: anything implementing Get and Commit is a Repository.
impl<T> Repository for T where T: Get + Commit {}
