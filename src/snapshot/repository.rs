use crate::aggregate::{hydrate, AggregateRepository};
use crate::entity::{upcast_events, Entity};
use crate::queued_repo::{GetAllWithOpts, GetWithOpts, ReadOpts, UnlockableRepository};
use crate::repository::{CommitBatch, Get, RepositoryError, SnapshotWrite, TransactionalCommit};

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
    R: TransactionalCommit,
    A: Snapshottable,
{
    /// Commit the aggregate and create a snapshot if the frequency threshold is met.
    pub fn commit(&self, aggregate: &mut A) -> Result<(), RepositoryError> {
        let snapshot = self.snapshot_record(aggregate)?;
        let snapshot_version = snapshot.as_ref().map(|record| record.version);
        let snapshots = snapshot.into_iter().map(SnapshotWrite::Save).collect();

        self.inner.repo().commit_batch(CommitBatch {
            entities: vec![aggregate.entity_mut()],
            outbox_messages: Vec::new(),
            read_model_plans: Vec::new(),
            snapshots,
        })?;

        if let Some(version) = snapshot_version {
            aggregate.entity_mut().set_snapshot_version(version);
        }
        Ok(())
    }

    /// Commit multiple aggregates and create snapshots where thresholds are met.
    pub fn commit_all(&self, aggregates: &mut [&mut A]) -> Result<(), RepositoryError> {
        let mut snapshot_versions = Vec::with_capacity(aggregates.len());
        let mut snapshots = Vec::new();
        for aggregate in aggregates.iter() {
            let snapshot = self.snapshot_record(*aggregate)?;
            snapshot_versions.push(snapshot.as_ref().map(|record| record.version));
            if let Some(record) = snapshot {
                snapshots.push(SnapshotWrite::Save(record));
            }
        }

        let entities: Vec<&mut Entity> = aggregates
            .iter_mut()
            .map(|agg| (*agg).entity_mut())
            .collect();
        self.inner.repo().commit_batch(CommitBatch {
            entities,
            outbox_messages: Vec::new(),
            read_model_plans: Vec::new(),
            snapshots,
        })?;

        for (aggregate, snapshot_version) in aggregates.iter_mut().zip(snapshot_versions) {
            if let Some(version) = snapshot_version {
                aggregate.entity_mut().set_snapshot_version(version);
            }
        }
        Ok(())
    }

    fn snapshot_record(&self, aggregate: &A) -> Result<Option<SnapshotRecord>, RepositoryError> {
        let version = aggregate.entity().version();
        let snap_version = aggregate.entity().snapshot_version();

        if version >= snap_version + self.frequency {
            let snap = aggregate.create_snapshot();
            let data = bitcode::serialize(&snap)
                .map_err(|e| RepositoryError::Replay(format!("snapshot serialize: {e}")))?;

            return Ok(Some(SnapshotRecord {
                aggregate_id: aggregate.entity().id().to_string(),
                version,
                data,
            }));
        }
        Ok(None)
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
    R: TransactionalCommit,
    A: Snapshottable,
{
    /// Start an outbox commit chain, same as AggregateRepository.
    pub fn outbox<'a>(
        &'a self,
        outbox: crate::outbox::OutboxMessage,
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
    outbox: crate::outbox::OutboxMessage,
}

impl<'a, R, A> SnapshotOutboxCommit<'a, R, A>
where
    R: TransactionalCommit,
    A: Snapshottable,
{
    pub fn commit(mut self, aggregate: &mut A) -> Result<(), RepositoryError> {
        let snapshot = self.snap_repo.snapshot_record(aggregate)?;
        let snapshot_version = snapshot.as_ref().map(|record| record.version);
        let snapshots = snapshot.into_iter().map(SnapshotWrite::Save).collect();
        self.outbox.set_source(aggregate);

        let mut batch = CommitBatch {
            entities: vec![aggregate.entity_mut()],
            outbox_messages: Vec::new(),
            read_model_plans: Vec::new(),
            snapshots,
        };
        batch.outbox_messages.push(self.outbox);
        self.snap_repo.inner.repo().commit_batch(batch)?;

        if let Some(version) = snapshot_version {
            aggregate.entity_mut().set_snapshot_version(version);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{impl_aggregate, AggregateRepository, Entity, EventRecord};
    use std::cell::RefCell;

    #[derive(Default)]
    struct TestAggregate {
        entity: Entity,
        value: u32,
    }

    impl TestAggregate {
        fn touch(&mut self) {
            if self.entity.id().is_empty() {
                self.entity.set_id("snap-1");
            }
            self.value += 1;
            self.entity.digest_empty("Touched").unwrap();
        }

        fn replay(&mut self, _event: &EventRecord) -> Result<(), String> {
            Ok(())
        }
    }

    impl_aggregate!(TestAggregate, entity, replay);

    impl Snapshottable for TestAggregate {
        type Snapshot = u32;

        fn create_snapshot(&self) -> Self::Snapshot {
            self.value
        }

        fn restore_from_snapshot(&mut self, snapshot: Self::Snapshot) {
            self.value = snapshot;
        }
    }

    #[derive(Default)]
    struct FailingSnapshotRepo {
        saw_snapshot: RefCell<bool>,
    }

    impl TransactionalCommit for FailingSnapshotRepo {
        fn commit_batch(&self, batch: CommitBatch<'_>) -> Result<(), RepositoryError> {
            if !batch.snapshots.is_empty() {
                *self.saw_snapshot.borrow_mut() = true;
                return Err(RepositoryError::Model("snapshot write failed".into()));
            }

            for entity in batch.entities {
                entity.mark_committed();
            }
            Ok(())
        }
    }

    #[test]
    fn snapshot_batch_failure_leaves_aggregate_uncommitted() {
        let repo = FailingSnapshotRepo::default();
        let aggregate_repo = AggregateRepository::new(repo);
        let snapshot_repo = SnapshotAggregateRepository::new(aggregate_repo, 1);

        let mut aggregate = TestAggregate::default();
        aggregate.touch();

        let err = snapshot_repo.commit(&mut aggregate).unwrap_err();

        assert_eq!(err, RepositoryError::Model("snapshot write failed".into()));
        assert!(*snapshot_repo.repo().repo().saw_snapshot.borrow());
        assert_eq!(aggregate.entity.committed_version(), 0);
        assert_eq!(aggregate.entity.snapshot_version(), 0);
        assert_eq!(aggregate.entity.new_events().len(), 1);
    }
}
