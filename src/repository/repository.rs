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

/// Scan all hydrated entity streams and return entities matching a predicate.
///
/// This is an in-process predicate scan. Implementations may optimize how they
/// enumerate streams, but the predicate is ordinary Rust code and is not a
/// query specification that a durable repository is expected to push down into
/// SQL. Production query workloads should use point lookups, read models, or
/// repository-specific indexed query APIs.
pub trait Scan {
    fn scan<F>(&self, predicate: F) -> Result<Vec<Entity>, RepositoryError>
    where
        F: Fn(&Entity) -> bool;
}

/// Scan hydrated entity streams and return the first entity matching a predicate.
pub trait ScanOne {
    fn scan_one<F>(&self, predicate: F) -> Result<Option<Entity>, RepositoryError>
    where
        F: Fn(&Entity) -> bool;
}

/// Scan hydrated entity streams and check whether any entity matches a predicate.
pub trait ScanExists {
    fn scan_exists<F>(&self, predicate: F) -> Result<bool, RepositoryError>
    where
        F: Fn(&Entity) -> bool;
}

/// Scan hydrated entity streams and count entities matching a predicate.
pub trait ScanCount {
    fn scan_count<F>(&self, predicate: F) -> Result<usize, RepositoryError>
    where
        F: Fn(&Entity) -> bool;
}

/// Compatibility trait for the historical `find` name.
///
/// `find` is an alias for [`Scan::scan`]. It remains available for existing
/// code, but new repository contracts should use `Scan` when they mean a full
/// predicate scan.
pub trait Find: Scan {
    fn find<F>(&self, predicate: F) -> Result<Vec<Entity>, RepositoryError>
    where
        F: Fn(&Entity) -> bool,
    {
        self.scan(predicate)
    }
}

impl<T: Scan> Find for T {}

/// Compatibility trait for the historical `find_one` name.
pub trait FindOne: ScanOne {
    fn find_one<F>(&self, predicate: F) -> Result<Option<Entity>, RepositoryError>
    where
        F: Fn(&Entity) -> bool,
    {
        self.scan_one(predicate)
    }
}

impl<T: ScanOne> FindOne for T {}

/// Compatibility trait for the historical `exists` name.
pub trait Exists: ScanExists {
    fn exists<F>(&self, predicate: F) -> Result<bool, RepositoryError>
    where
        F: Fn(&Entity) -> bool,
    {
        self.scan_exists(predicate)
    }
}

impl<T: ScanExists> Exists for T {}

/// Compatibility trait for the historical `count` name.
pub trait Count: ScanCount {
    fn count<F>(&self, predicate: F) -> Result<usize, RepositoryError>
    where
        F: Fn(&Entity) -> bool,
    {
        self.scan_count(predicate)
    }
}

impl<T: ScanCount> Count for T {}

use crate::entity::Committable;

/// Append new aggregate event records for one or more entities.
pub trait Commit {
    fn commit<C: Committable + ?Sized>(&self, committable: &mut C) -> Result<(), RepositoryError>;
}

/// Full repository trait combining point lookups, explicit scans, and commits.
///
/// `Scan` capabilities are part of the compatibility surface, but they are
/// enumeration semantics. Durable repositories can expose additional indexed
/// query traits without changing the meaning of this core contract.
pub trait Repository: Get + Scan + ScanOne + ScanExists + ScanCount + Commit {}

// Blanket implementation: anything implementing all traits is a Repository
impl<T> Repository for T where T: Get + Scan + ScanOne + ScanExists + ScanCount + Commit {}
