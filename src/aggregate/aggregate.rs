use std::fmt;
use std::marker::PhantomData;

use crate::entity::{upcast_events, Entity, EventRecord, EventUpcaster};
use crate::queued_repo::{GetAllWithOpts, GetWithOpts, ReadOpts, UnlockableRepository};
use crate::repository::{
    Commit, CommitBatch, Get, Repository, RepositoryError, Scan, TransactionalCommit,
};
use crate::snapshot::{SnapshotAggregateRepository, SnapshotStore, Snapshottable};

/// Trait for domain aggregates that can be event-sourced.
pub trait Aggregate: Sized + Default {
    type ReplayError: fmt::Display;

    fn new_empty() -> Self {
        Self::default()
    }
    fn entity(&self) -> &Entity;
    fn entity_mut(&mut self) -> &mut Entity;
    fn replay_event(&mut self, event: &EventRecord) -> Result<(), Self::ReplayError>;

    /// Override to register upcasters for this aggregate's events.
    /// Upcasters are configuration, not state — this is a static method.
    fn upcasters() -> &'static [EventUpcaster] {
        &[]
    }
}

#[macro_export]
macro_rules! impl_aggregate {
    ($ty:ty, $entity:ident, $replay:ident) => {
        $crate::impl_aggregate!($ty, $entity, $replay, String);
    };
    ($ty:ty, $entity:ident, $replay:ident, $err:ty) => {
        impl $crate::Aggregate for $ty {
            type ReplayError = $err;

            fn entity(&self) -> &$crate::Entity {
                &self.$entity
            }

            fn entity_mut(&mut self) -> &mut $crate::Entity {
                &mut self.$entity
            }

            fn replay_event(
                &mut self,
                event: &$crate::EventRecord,
            ) -> Result<(), Self::ReplayError> {
                Self::$replay(self, event)
            }
        }
    };
}

// Note: The `aggregate!` macro is now provided as a proc-macro from sourced_rust_macros.
// It generates the event enum, TryFrom impl, apply method, and Aggregate trait impl.
// Use: aggregate!(MyAggregate, entity_field { "EventName"(args) => method_name, ... });

/// Hydrate an aggregate from an entity by replaying its events.
pub fn hydrate<A: Aggregate>(entity: Entity) -> Result<A, RepositoryError> {
    let mut agg = A::new_empty();
    *agg.entity_mut() = entity;

    let upcasters = A::upcasters();
    let events = if upcasters.is_empty() {
        agg.entity().events().to_vec()
    } else {
        upcast_events(agg.entity().events().to_vec(), upcasters)
            .map_err(|err| RepositoryError::Replay(err.to_string()))?
    };

    agg.entity_mut().set_replaying(true);
    for event in &events {
        if let Err(err) = agg.replay_event(event) {
            agg.entity_mut().set_replaying(false);
            return Err(RepositoryError::Replay(err.to_string()));
        }
    }
    agg.entity_mut().set_replaying(false);

    Ok(agg)
}

/// Extension trait adding aggregate-aware get method.
pub trait GetAggregate: Get {
    fn get_aggregate<A: Aggregate>(&self, id: &str) -> Result<Option<A>, RepositoryError>
    where
        Self: Sized,
    {
        let entity = self.get(id)?;
        let Some(entity) = entity else {
            return Ok(None);
        };
        Ok(Some(hydrate::<A>(entity)?))
    }
}

impl<R: Get> GetAggregate for R {}

/// Extension trait adding aggregate-aware get_all method.
pub trait GetAllAggregates: Get {
    fn get_all_aggregates<A: Aggregate>(&self, ids: &[&str]) -> Result<Vec<A>, RepositoryError>
    where
        Self: Sized,
    {
        let entities = self.get(ids)?;
        let mut aggregates = Vec::with_capacity(entities.len());
        for entity in entities {
            aggregates.push(hydrate::<A>(entity)?);
        }
        Ok(aggregates)
    }
}

impl<R: Get> GetAllAggregates for R {}

/// Extension trait adding aggregate-aware commit methods.
pub trait CommitAggregate: Commit {
    fn commit_aggregate<A: Aggregate>(&self, aggregate: &mut A) -> Result<(), RepositoryError> {
        self.commit(aggregate.entity_mut())
    }

    fn commit_all_aggregates<A: Aggregate>(
        &self,
        aggregates: &mut [&mut A],
    ) -> Result<(), RepositoryError>
    where
        Self: TransactionalCommit,
    {
        let entities: Vec<&mut Entity> = aggregates
            .iter_mut()
            .map(|agg| (*agg).entity_mut())
            .collect();
        self.commit_batch(CommitBatch::new(entities))
    }
}

impl<R: Commit> CommitAggregate for R {}

/// Extension trait adding aggregate-aware scan methods.
pub trait ScanAggregate: Scan {
    fn scan_aggregate<A: Aggregate, F>(&self, predicate: F) -> Result<Vec<A>, RepositoryError>
    where
        F: Fn(&A) -> bool,
    {
        let entities = self.scan(|_| true)?;
        let mut results = Vec::new();
        for entity in entities {
            let agg = hydrate::<A>(entity)?;
            if predicate(&agg) {
                results.push(agg);
            }
        }
        Ok(results)
    }
}

impl<R: Scan> ScanAggregate for R {}

/// Extension trait adding aggregate-aware scan_one methods.
pub trait ScanOneAggregate: Scan {
    fn scan_one_aggregate<A: Aggregate, F>(
        &self,
        predicate: F,
    ) -> Result<Option<A>, RepositoryError>
    where
        F: Fn(&A) -> bool,
    {
        let entities = self.scan(|_| true)?;
        for entity in entities {
            let agg = hydrate::<A>(entity)?;
            if predicate(&agg) {
                return Ok(Some(agg));
            }
        }
        Ok(None)
    }
}

impl<R: Scan> ScanOneAggregate for R {}

/// Extension trait adding aggregate-aware scan_exists method.
pub trait ScanExistsAggregate: Scan {
    fn scan_exists_aggregate<A: Aggregate, F>(&self, predicate: F) -> Result<bool, RepositoryError>
    where
        F: Fn(&A) -> bool,
    {
        let entities = self.scan(|_| true)?;
        for entity in entities {
            let agg = hydrate::<A>(entity)?;
            if predicate(&agg) {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

impl<R: Scan> ScanExistsAggregate for R {}

/// Extension trait adding aggregate-aware scan_count method.
pub trait ScanCountAggregate: Scan {
    fn scan_count_aggregate<A: Aggregate, F>(&self, predicate: F) -> Result<usize, RepositoryError>
    where
        F: Fn(&A) -> bool,
    {
        let entities = self.scan(|_| true)?;
        let mut count = 0;
        for entity in entities {
            let agg = hydrate::<A>(entity)?;
            if predicate(&agg) {
                count += 1;
            }
        }
        Ok(count)
    }
}

impl<R: Scan> ScanCountAggregate for R {}

/// Compatibility trait for the historical aggregate `find` name.
pub trait FindAggregate: ScanAggregate {
    fn find_aggregate<A: Aggregate, F>(&self, predicate: F) -> Result<Vec<A>, RepositoryError>
    where
        F: Fn(&A) -> bool,
    {
        self.scan_aggregate(predicate)
    }
}

impl<R: ScanAggregate> FindAggregate for R {}

/// Compatibility trait for the historical aggregate `find_one` name.
pub trait FindOneAggregate: ScanOneAggregate {
    fn find_one_aggregate<A: Aggregate, F>(
        &self,
        predicate: F,
    ) -> Result<Option<A>, RepositoryError>
    where
        F: Fn(&A) -> bool,
    {
        self.scan_one_aggregate(predicate)
    }
}

impl<R: ScanOneAggregate> FindOneAggregate for R {}

/// Compatibility trait for the historical aggregate `exists` name.
pub trait ExistsAggregate: ScanExistsAggregate {
    fn exists_aggregate<A: Aggregate, F>(&self, predicate: F) -> Result<bool, RepositoryError>
    where
        F: Fn(&A) -> bool,
    {
        self.scan_exists_aggregate(predicate)
    }
}

impl<R: ScanExistsAggregate> ExistsAggregate for R {}

/// Compatibility trait for the historical aggregate `count` name.
pub trait CountAggregate: ScanCountAggregate {
    fn count_aggregate<A: Aggregate, F>(&self, predicate: F) -> Result<usize, RepositoryError>
    where
        F: Fn(&A) -> bool,
    {
        self.scan_count_aggregate(predicate)
    }
}

impl<R: ScanCountAggregate> CountAggregate for R {}

/// Combined extension trait for full repository aggregate support.
pub trait RepositoryExt:
    GetAggregate
    + GetAllAggregates
    + CommitAggregate
    + ScanAggregate
    + ScanOneAggregate
    + ScanExistsAggregate
    + ScanCountAggregate
    + FindAggregate
    + FindOneAggregate
    + ExistsAggregate
    + CountAggregate
{
}

impl<R: Repository> RepositoryExt for R {}

/// Builder trait for creating typed aggregate repositories.
pub trait AggregateBuilder: Sized {
    fn aggregate<A: Aggregate>(self) -> AggregateRepository<Self, A> {
        AggregateRepository::new(self)
    }
}

impl<T> AggregateBuilder for T {}

/// A repository wrapper that provides typed access to a specific aggregate type.
pub struct AggregateRepository<R, A> {
    repo: R,
    _marker: PhantomData<A>,
}

impl<R, A> AggregateRepository<R, A> {
    pub fn new(repo: R) -> Self {
        AggregateRepository {
            repo,
            _marker: PhantomData,
        }
    }

    pub fn repo(&self) -> &R {
        &self.repo
    }

    pub fn repo_mut(&mut self) -> &mut R {
        &mut self.repo
    }
}

impl<R, A> AggregateRepository<R, A>
where
    R: Get,
    A: Aggregate,
{
    pub fn get(&self, id: &str) -> Result<Option<A>, RepositoryError> {
        let entity = self.repo.get(id)?;
        let Some(entity) = entity else {
            return Ok(None);
        };
        Ok(Some(hydrate::<A>(entity)?))
    }
}

impl<R, A> AggregateRepository<R, A>
where
    R: Get,
    A: Aggregate,
{
    pub fn get_all(&self, ids: &[&str]) -> Result<Vec<A>, RepositoryError> {
        let entities = self.repo.get(ids)?;
        let mut aggregates = Vec::with_capacity(entities.len());
        for entity in entities {
            aggregates.push(hydrate::<A>(entity)?);
        }
        Ok(aggregates)
    }
}

impl<R, A> AggregateRepository<R, A>
where
    R: Commit,
    A: Aggregate,
{
    pub fn commit(&self, aggregate: &mut A) -> Result<(), RepositoryError> {
        self.repo.commit(aggregate.entity_mut())
    }
}

impl<R, A> AggregateRepository<R, A>
where
    R: TransactionalCommit,
    A: Aggregate,
{
    pub fn commit_all(&self, aggregates: &mut [&mut A]) -> Result<(), RepositoryError> {
        let entities: Vec<&mut Entity> = aggregates
            .iter_mut()
            .map(|agg| (*agg).entity_mut())
            .collect();
        self.repo.commit_batch(CommitBatch::new(entities))
    }
}

impl<R, A> AggregateRepository<R, A>
where
    R: Scan,
    A: Aggregate,
{
    /// Scan all aggregate streams and return aggregates matching a predicate.
    pub fn scan<F>(&self, predicate: F) -> Result<Vec<A>, RepositoryError>
    where
        F: Fn(&A) -> bool,
    {
        let entities = self.repo.scan(|_| true)?;
        let mut results = Vec::new();
        for entity in entities {
            let agg = hydrate::<A>(entity)?;
            if predicate(&agg) {
                results.push(agg);
            }
        }
        Ok(results)
    }

    /// Scan aggregate streams and return the first aggregate matching a predicate.
    pub fn scan_one<F>(&self, predicate: F) -> Result<Option<A>, RepositoryError>
    where
        F: Fn(&A) -> bool,
    {
        let entities = self.repo.scan(|_| true)?;
        for entity in entities {
            let agg = hydrate::<A>(entity)?;
            if predicate(&agg) {
                return Ok(Some(agg));
            }
        }
        Ok(None)
    }

    /// Scan aggregate streams and check whether any aggregate matches a predicate.
    pub fn scan_exists<F>(&self, predicate: F) -> Result<bool, RepositoryError>
    where
        F: Fn(&A) -> bool,
    {
        Ok(self.scan_one(predicate)?.is_some())
    }

    /// Scan aggregate streams and count aggregates matching a predicate.
    pub fn scan_count<F>(&self, predicate: F) -> Result<usize, RepositoryError>
    where
        F: Fn(&A) -> bool,
    {
        Ok(self.scan(predicate)?.len())
    }

    /// Compatibility alias for [`Self::scan`].
    pub fn find<F>(&self, predicate: F) -> Result<Vec<A>, RepositoryError>
    where
        F: Fn(&A) -> bool,
    {
        self.scan(predicate)
    }

    /// Compatibility alias for [`Self::scan_one`].
    pub fn find_one<F>(&self, predicate: F) -> Result<Option<A>, RepositoryError>
    where
        F: Fn(&A) -> bool,
    {
        self.scan_one(predicate)
    }

    /// Compatibility alias for [`Self::scan_exists`].
    pub fn exists<F>(&self, predicate: F) -> Result<bool, RepositoryError>
    where
        F: Fn(&A) -> bool,
    {
        self.scan_exists(predicate)
    }

    /// Compatibility alias for [`Self::scan_count`].
    pub fn count<F>(&self, predicate: F) -> Result<usize, RepositoryError>
    where
        F: Fn(&A) -> bool,
    {
        self.scan_count(predicate)
    }
}

impl<R, A> AggregateRepository<R, A>
where
    R: UnlockableRepository,
    A: Aggregate,
{
    pub fn abort(&self, aggregate: &A) -> Result<(), RepositoryError> {
        self.repo.unlock(aggregate.entity().id())
    }
}

impl<R, A> AggregateRepository<R, A>
where
    R: SnapshotStore,
    A: Snapshottable,
{
    /// Wrap this repository with snapshot support at the given event frequency.
    pub fn with_snapshots(self, frequency: u64) -> SnapshotAggregateRepository<R, A> {
        SnapshotAggregateRepository::new(self, frequency)
    }
}

impl<R, A> AggregateRepository<R, A>
where
    R: GetWithOpts,
    A: Aggregate,
{
    /// Get an aggregate with options (e.g., to skip locking).
    pub fn get_with(&self, id: &str, opts: ReadOpts) -> Result<Option<A>, RepositoryError> {
        let entity = self.repo.get_with(id, opts)?;
        let Some(entity) = entity else {
            return Ok(None);
        };
        Ok(Some(hydrate::<A>(entity)?))
    }

    /// Non-locking read (alias for get_with no_lock).
    pub fn peek(&self, id: &str) -> Result<Option<A>, RepositoryError> {
        self.get_with(id, ReadOpts::no_lock())
    }
}

impl<R, A> AggregateRepository<R, A>
where
    R: GetAllWithOpts,
    A: Aggregate,
{
    /// Get all aggregates with options (e.g., to skip locking).
    pub fn get_all_with(&self, ids: &[&str], opts: ReadOpts) -> Result<Vec<A>, RepositoryError> {
        let entities = self.repo.get_all_with(ids, opts)?;
        let mut aggregates = Vec::with_capacity(entities.len());
        for entity in entities {
            aggregates.push(hydrate::<A>(entity)?);
        }
        Ok(aggregates)
    }

    /// Non-locking read (alias for get_all_with no_lock).
    pub fn peek_all(&self, ids: &[&str]) -> Result<Vec<A>, RepositoryError> {
        self.get_all_with(ids, ReadOpts::no_lock())
    }
}
