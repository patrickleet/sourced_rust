use crate::aggregate::{hydrate, AggregateRepository};
use crate::entity::{Entity, upcast_events};
use crate::repository::{Commit, Find, Get, RepositoryError};
use crate::queued_repo::{GetWithOpts, GetAllWithOpts, ReadOpts, UnlockableRepository};

use super::snapshottable::Snapshottable;
use super::store::{SnapshotRecord, SnapshotStore};

/// Hydrate an aggregate from a snapshot, replaying only events after the snapshot version.
pub fn hydrate_from_snapshot<A: Snapshottable>(
    entity: Entity,
    snapshot: SnapshotRecord,
) -> Result<A, RepositoryError> {
    let mut agg = A::new_empty();
    *agg.entity_mut() = entity;

    // Set snapshot_version so frequency check works on next commit
    agg.entity_mut().set_snapshot_version(snapshot.version);

    // Restore aggregate state from snapshot
    let snap: A::Snapshot = bitcode::deserialize(&snapshot.data)
        .map_err(|e| RepositoryError::Replay(format!("snapshot deserialize: {e}")))?;
    agg.restore_from_snapshot(snap);

    // Replay only events AFTER the snapshot
    let post_snapshot: Vec<crate::entity::EventRecord> = agg
        .entity()
        .events()
        .iter()
        .filter(|e| e.sequence > snapshot.version)
        .cloned()
        .collect();

    // Apply upcasters to post-snapshot events
    let upcasters = A::upcasters();
    let events = if upcasters.is_empty() {
        post_snapshot
    } else {
        upcast_events(post_snapshot, upcasters)
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

/// A repository wrapper that provides snapshot-aware get and commit for a specific aggregate type.
pub struct SnapshotAggregateRepository<R, A> {
    inner: AggregateRepository<R, A>,
    frequency: u64,
}

impl<R, A> SnapshotAggregateRepository<R, A> {
    pub fn new(inner: AggregateRepository<R, A>, frequency: u64) -> Self {
        SnapshotAggregateRepository { inner, frequency }
    }

    /// Access the inner AggregateRepository.
    pub fn repo(&self) -> &AggregateRepository<R, A> {
        &self.inner
    }
}

// ============================================================================
// get / get_all — snapshot-aware hydration
// ============================================================================

impl<R, A> SnapshotAggregateRepository<R, A>
where
    R: Get + SnapshotStore,
    A: Snapshottable,
{
    /// Load an aggregate, using a snapshot if available.
    pub fn get(&self, id: &str) -> Result<Option<A>, RepositoryError> {
        let entity = self.inner.repo().get(id)?;
        let Some(entity) = entity else {
            return Ok(None);
        };
        let snapshot = self.inner.repo().get_snapshot(id)?;
        Ok(Some(self.hydrate_with_optional_snapshot(entity, snapshot)?))
    }

    /// Load multiple aggregates by ID.
    pub fn get_all(&self, ids: &[&str]) -> Result<Vec<A>, RepositoryError> {
        let entities = self.inner.repo().get(ids)?;
        let mut aggregates = Vec::with_capacity(entities.len());
        for entity in entities {
            let snapshot = self.inner.repo().get_snapshot(entity.id())?;
            aggregates.push(self.hydrate_with_optional_snapshot(entity, snapshot)?);
        }
        Ok(aggregates)
    }

    fn hydrate_with_optional_snapshot(
        &self,
        entity: Entity,
        snapshot: Option<SnapshotRecord>,
    ) -> Result<A, RepositoryError> {
        match snapshot {
            Some(snap) if snap.version <= entity.version() => {
                hydrate_from_snapshot::<A>(entity, snap)
            }
            _ => hydrate::<A>(entity),
        }
    }
}

// ============================================================================
// commit / commit_all — auto-snapshot after threshold
// ============================================================================

impl<R, A> SnapshotAggregateRepository<R, A>
where
    R: Commit + SnapshotStore,
    A: Snapshottable,
{
    /// Commit the aggregate and create a snapshot if the frequency threshold is met.
    pub fn commit(&self, aggregate: &mut A) -> Result<(), RepositoryError> {
        self.inner.repo().commit(aggregate.entity_mut())?;
        self.maybe_snapshot(aggregate)?;
        Ok(())
    }

    /// Commit multiple aggregates and create snapshots where thresholds are met.
    pub fn commit_all(&self, aggregates: &mut [&mut A]) -> Result<(), RepositoryError> {
        let mut entities: Vec<&mut Entity> = aggregates
            .iter_mut()
            .map(|agg| (*agg).entity_mut())
            .collect();
        self.inner.repo().commit(&mut entities[..])?;

        for agg in aggregates.iter_mut() {
            self.maybe_snapshot(*agg)?;
        }
        Ok(())
    }

    fn maybe_snapshot(&self, aggregate: &mut A) -> Result<(), RepositoryError> {
        let version = aggregate.entity().version();
        let snap_version = aggregate.entity().snapshot_version();

        if version >= snap_version + self.frequency {
            let snap = aggregate.create_snapshot();
            let data = bitcode::serialize(&snap)
                .map_err(|e| RepositoryError::Replay(format!("snapshot serialize: {e}")))?;

            self.inner.repo().save_snapshot(SnapshotRecord {
                aggregate_id: aggregate.entity().id().to_string(),
                version,
                data,
            })?;

            aggregate.entity_mut().set_snapshot_version(version);
        }
        Ok(())
    }
}

// ============================================================================
// find / find_one / exists / count — delegate with snapshot-aware hydration
// ============================================================================

impl<R, A> SnapshotAggregateRepository<R, A>
where
    R: Find + SnapshotStore,
    A: Snapshottable,
{
    pub fn find<F>(&self, predicate: F) -> Result<Vec<A>, RepositoryError>
    where
        F: Fn(&A) -> bool,
    {
        let entities = self.inner.repo().find(|_| true)?;
        let mut results = Vec::new();
        for entity in entities {
            let snapshot = self.inner.repo().get_snapshot(entity.id())?;
            let agg = match snapshot {
                Some(snap) if snap.version <= entity.version() => {
                    hydrate_from_snapshot::<A>(entity, snap)?
                }
                _ => hydrate::<A>(entity)?,
            };
            if predicate(&agg) {
                results.push(agg);
            }
        }
        Ok(results)
    }

    pub fn find_one<F>(&self, predicate: F) -> Result<Option<A>, RepositoryError>
    where
        F: Fn(&A) -> bool,
    {
        let entities = self.inner.repo().find(|_| true)?;
        for entity in entities {
            let snapshot = self.inner.repo().get_snapshot(entity.id())?;
            let agg = match snapshot {
                Some(snap) if snap.version <= entity.version() => {
                    hydrate_from_snapshot::<A>(entity, snap)?
                }
                _ => hydrate::<A>(entity)?,
            };
            if predicate(&agg) {
                return Ok(Some(agg));
            }
        }
        Ok(None)
    }

    pub fn exists<F>(&self, predicate: F) -> Result<bool, RepositoryError>
    where
        F: Fn(&A) -> bool,
    {
        Ok(self.find_one(predicate)?.is_some())
    }

    pub fn count<F>(&self, predicate: F) -> Result<usize, RepositoryError>
    where
        F: Fn(&A) -> bool,
    {
        Ok(self.find(predicate)?.len())
    }
}

// ============================================================================
// abort / peek — delegate through inner AggregateRepository
// ============================================================================

impl<R, A> SnapshotAggregateRepository<R, A>
where
    R: UnlockableRepository,
    A: Snapshottable,
{
    pub fn abort(&self, aggregate: &A) -> Result<(), RepositoryError> {
        self.inner.repo().unlock(aggregate.entity().id())
    }
}

impl<R, A> SnapshotAggregateRepository<R, A>
where
    R: GetWithOpts + SnapshotStore,
    A: Snapshottable,
{
    /// Non-locking read with snapshot-aware hydration.
    pub fn peek(&self, id: &str) -> Result<Option<A>, RepositoryError> {
        let entity = self.inner.repo().get_with(id, ReadOpts::no_lock())?;
        let Some(entity) = entity else {
            return Ok(None);
        };
        let snapshot = self.inner.repo().get_snapshot(id)?;
        match snapshot {
            Some(snap) if snap.version <= entity.version() => {
                Ok(Some(hydrate_from_snapshot::<A>(entity, snap)?))
            }
            _ => Ok(Some(hydrate::<A>(entity)?)),
        }
    }
}

impl<R, A> SnapshotAggregateRepository<R, A>
where
    R: GetAllWithOpts + SnapshotStore,
    A: Snapshottable,
{
    /// Non-locking bulk read with snapshot-aware hydration.
    pub fn peek_all(&self, ids: &[&str]) -> Result<Vec<A>, RepositoryError> {
        let entities = self.inner.repo().get_all_with(ids, ReadOpts::no_lock())?;
        let mut aggregates = Vec::with_capacity(entities.len());
        for entity in entities {
            let snapshot = self.inner.repo().get_snapshot(entity.id())?;
            let agg = match snapshot {
                Some(snap) if snap.version <= entity.version() => {
                    hydrate_from_snapshot::<A>(entity, snap)?
                }
                _ => hydrate::<A>(entity)?,
            };
            aggregates.push(agg);
        }
        Ok(aggregates)
    }
}

// ============================================================================
// Outbox integration — delegate through inner AggregateRepository
// ============================================================================

impl<R, A> SnapshotAggregateRepository<R, A>
where
    R: Commit + SnapshotStore,
    A: Snapshottable,
{
    /// Start an outbox commit chain, same as AggregateRepository.
    pub fn outbox<'a>(
        &'a self,
        outbox: &'a mut crate::outbox::OutboxMessage,
    ) -> SnapshotOutboxCommit<'a, R, A> {
        SnapshotOutboxCommit {
            snap_repo: self,
            outbox,
        }
    }
}

/// Helper for chaining outbox + snapshot-aware commit.
pub struct SnapshotOutboxCommit<'a, R, A> {
    snap_repo: &'a SnapshotAggregateRepository<R, A>,
    outbox: &'a mut crate::outbox::OutboxMessage,
}

impl<'a, R, A> SnapshotOutboxCommit<'a, R, A>
where
    R: Commit + SnapshotStore,
    A: Snapshottable,
{
    pub fn commit(self, aggregate: &mut A) -> Result<(), RepositoryError> {
        self.snap_repo.inner.repo().commit(
            &mut [aggregate.entity_mut(), self.outbox.entity_mut()][..],
        )?;
        self.snap_repo.maybe_snapshot(aggregate)?;
        Ok(())
    }
}
