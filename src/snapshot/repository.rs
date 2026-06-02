use crate::aggregate::{hydrate, AggregateRepository};
use crate::entity::{upcast_events, Entity};
use crate::repository::{
    CommitBatch, GetStream, RepositoryError, SnapshotStore, SnapshotWrite, StreamIdentity,
    StreamWrite, TransactionalCommit,
};

use super::snapshottable::Snapshottable;
use super::store::SnapshotRecord;

#[derive(Debug, PartialEq, Eq)]
enum SnapshotHydrationError {
    Cache(String),
    Replay(String),
}

fn snapshot_hydration_error_to_repository_error(err: SnapshotHydrationError) -> RepositoryError {
    match err {
        SnapshotHydrationError::Cache(message) | SnapshotHydrationError::Replay(message) => {
            RepositoryError::Replay(message)
        }
    }
}

fn snapshot_due(version: u64, snapshot_version: u64, frequency: u64) -> bool {
    version.saturating_sub(snapshot_version) >= frequency
}

/// Hydrate an aggregate from a snapshot cache record, replaying only events
/// after the snapshot version.
pub fn hydrate_from_snapshot<A: Snapshottable>(
    entity: Entity,
    snapshot: SnapshotRecord,
) -> Result<A, RepositoryError> {
    let snapshot_payload = prepare_snapshot::<A>(&entity, &snapshot)
        .map_err(snapshot_hydration_error_to_repository_error)?;
    hydrate_prepared_snapshot::<A>(entity, &snapshot, snapshot_payload)
        .map_err(snapshot_hydration_error_to_repository_error)
}

fn entity_stream_version(entity: &Entity) -> u64 {
    entity
        .events()
        .iter()
        .map(|event| event.sequence)
        .max()
        .unwrap_or_else(|| entity.version())
}

fn validate_snapshot_for_entity<A: Snapshottable>(
    entity: &Entity,
    snapshot: &SnapshotRecord,
) -> Result<(), SnapshotHydrationError> {
    if snapshot.aggregate_id != entity.id() || snapshot.aggregate_type != A::aggregate_type() {
        return Err(SnapshotHydrationError::Cache(format!(
            "snapshot cache identity {}:{} does not match aggregate {}:{}",
            snapshot.aggregate_type,
            snapshot.aggregate_id,
            A::aggregate_type(),
            entity.id()
        )));
    }

    let stream_version = entity_stream_version(entity);
    if snapshot.version > stream_version {
        return Err(SnapshotHydrationError::Cache(format!(
            "snapshot cache version {} exceeds stream version {} for {}:{}",
            snapshot.version, stream_version, snapshot.aggregate_type, snapshot.aggregate_id
        )));
    }

    Ok(())
}

fn prepare_snapshot<A: Snapshottable>(
    entity: &Entity,
    snapshot: &SnapshotRecord,
) -> Result<A::Snapshot, SnapshotHydrationError> {
    validate_snapshot_for_entity::<A>(entity, snapshot)?;
    if !snapshot.has_supported_payload_codec() {
        return Err(SnapshotHydrationError::Cache(format!(
            "unsupported snapshot payload codec `{}` version {}",
            snapshot.payload_codec, snapshot.payload_codec_version
        )));
    }

    bitcode::deserialize(&snapshot.payload)
        .map_err(|e| SnapshotHydrationError::Cache(format!("snapshot deserialize: {e}")))
}

fn hydrate_prepared_snapshot<A: Snapshottable>(
    entity: Entity,
    snapshot: &SnapshotRecord,
    snapshot_payload: A::Snapshot,
) -> Result<A, SnapshotHydrationError> {
    let mut agg = A::new_empty();
    *agg.entity_mut() = entity;

    // Set snapshot_version so frequency check works on next commit
    agg.entity_mut().set_snapshot_version(snapshot.version);

    // Restore aggregate state from snapshot
    agg.restore_from_snapshot(snapshot_payload);

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
            .map_err(|err| SnapshotHydrationError::Replay(err.to_string()))?
    };

    agg.entity_mut().set_replaying(true);
    for event in &events {
        if let Err(err) = agg.replay_event(event) {
            agg.entity_mut().set_replaying(false);
            return Err(SnapshotHydrationError::Replay(err.to_string()));
        }
    }
    agg.entity_mut().set_replaying(false);
    Ok(agg)
}

fn snapshot_type_name<A: Snapshottable>() -> String {
    std::any::type_name::<A::Snapshot>().to_string()
}

fn snapshot_record_for<A: Snapshottable>(aggregate: &A) -> Result<SnapshotRecord, RepositoryError> {
    let payload = bitcode::serialize(&aggregate.create_snapshot())
        .map_err(|e| RepositoryError::Replay(format!("snapshot serialize: {e}")))?;

    Ok(SnapshotRecord::new(
        A::aggregate_type(),
        aggregate.entity().id(),
        aggregate.entity().version(),
        snapshot_type_name::<A>(),
        SnapshotRecord::DEFAULT_SNAPSHOT_VERSION,
        payload,
    ))
}

fn hydrate_with_optional_snapshot<A: Snapshottable>(
    entity: Entity,
    snapshot: Option<SnapshotRecord>,
) -> Result<A, RepositoryError> {
    let Some(snapshot) = snapshot else {
        return hydrate::<A>(entity);
    };

    let snapshot_payload = match prepare_snapshot::<A>(&entity, &snapshot) {
        Ok(snapshot_payload) => snapshot_payload,
        Err(SnapshotHydrationError::Cache(_)) => return hydrate::<A>(entity),
        Err(SnapshotHydrationError::Replay(message)) => {
            return Err(RepositoryError::Replay(message))
        }
    };

    hydrate_prepared_snapshot::<A>(entity, &snapshot, snapshot_payload)
        .map_err(snapshot_hydration_error_to_repository_error)
}

/// Async repository wrapper that treats aggregate snapshots as rebuildable
/// hydration cache records.
pub struct SnapshotAggregateRepository<R, A> {
    inner: AggregateRepository<R, A>,
    frequency: u64,
}

impl<R, A> SnapshotAggregateRepository<R, A> {
    pub fn new(inner: AggregateRepository<R, A>, frequency: u64) -> Self {
        Self { inner, frequency }
    }

    pub fn repo(&self) -> &AggregateRepository<R, A> {
        &self.inner
    }
}

impl<R, A> AggregateRepository<R, A> {
    /// Wrap this async repository with snapshot cache support at the given event
    /// frequency.
    pub fn with_snapshots(self, frequency: u64) -> SnapshotAggregateRepository<R, A> {
        SnapshotAggregateRepository::new(self, frequency)
    }
}

impl<R, A> SnapshotAggregateRepository<R, A>
where
    R: GetStream + SnapshotStore,
    A: Snapshottable + Send,
{
    pub async fn get(&self, id: &str) -> Result<Option<A>, RepositoryError> {
        let identity = StreamIdentity::new(A::aggregate_type(), id)?;
        let entity = self.inner.repo().get_stream(&identity).await?;
        let Some(entity) = entity else {
            return Ok(None);
        };
        let snapshot = self.inner.repo().get_snapshot(&identity).await?;
        Ok(Some(hydrate_with_optional_snapshot::<A>(entity, snapshot)?))
    }

    pub async fn get_all(&self, ids: &[&str]) -> Result<Vec<A>, RepositoryError> {
        let identities = ids
            .iter()
            .map(|id| StreamIdentity::new(A::aggregate_type(), *id))
            .collect::<Result<Vec<_>, _>>()?;
        let entities = self.inner.repo().get_streams(&identities).await?;
        let mut aggregates = Vec::with_capacity(entities.len());
        for entity in entities {
            let identity = StreamIdentity::new(A::aggregate_type(), entity.id())?;
            let snapshot = self.inner.repo().get_snapshot(&identity).await?;
            aggregates.push(hydrate_with_optional_snapshot::<A>(entity, snapshot)?);
        }
        Ok(aggregates)
    }
}

impl<R, A> SnapshotAggregateRepository<R, A>
where
    R: TransactionalCommit,
    A: Snapshottable + Send,
{
    pub async fn commit(&self, aggregate: &mut A) -> Result<(), RepositoryError> {
        let snapshot = self.snapshot_record(aggregate)?;
        let snapshot_version = snapshot.as_ref().map(|record| record.version);
        let identity = StreamIdentity::new(A::aggregate_type(), aggregate.entity().id())?;
        let snapshots = snapshot
            .into_iter()
            .map(|record| SnapshotWrite::Save {
                identity: identity.clone(),
                record,
            })
            .collect();

        self.inner
            .repo()
            .commit_batch(CommitBatch {
                streams: vec![StreamWrite::new(identity, aggregate.entity_mut())],
                outbox_messages: Vec::new(),
                read_model_plans: Vec::new(),
                snapshots,
                inbox_receipts: Vec::new(),
            })
            .await?;

        if let Some(version) = snapshot_version {
            aggregate.entity_mut().set_snapshot_version(version);
        }
        Ok(())
    }

    pub async fn commit_all(&self, aggregates: &mut [&mut A]) -> Result<(), RepositoryError> {
        let mut snapshot_versions = Vec::with_capacity(aggregates.len());
        let mut snapshots = Vec::new();
        for aggregate in aggregates.iter() {
            let snapshot = self.snapshot_record(*aggregate)?;
            snapshot_versions.push(snapshot.as_ref().map(|record| record.version));
            if let Some(record) = snapshot {
                snapshots.push(SnapshotWrite::Save {
                    identity: StreamIdentity::new(
                        A::aggregate_type(),
                        record.aggregate_id.as_str(),
                    )?,
                    record,
                });
            }
        }

        let mut streams = Vec::with_capacity(aggregates.len());
        for aggregate in aggregates.iter_mut() {
            let identity = StreamIdentity::new(A::aggregate_type(), (*aggregate).entity().id())?;
            streams.push(StreamWrite::new(identity, (*aggregate).entity_mut()));
        }

        self.inner
            .repo()
            .commit_batch(CommitBatch {
                streams,
                outbox_messages: Vec::new(),
                read_model_plans: Vec::new(),
                snapshots,
                inbox_receipts: Vec::new(),
            })
            .await?;

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

        if snapshot_due(version, snap_version, self.frequency) {
            return snapshot_record_for(aggregate).map(Some);
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{sourced, Aggregate, EventRecord};

    #[derive(Default)]
    struct TestAggregate {
        entity: Entity,
        value: u32,
    }

    #[sourced(entity)]
    impl TestAggregate {
        #[event("Touched")]
        fn touch(&mut self) {
            if self.entity.id().is_empty() {
                self.entity.set_id("snap-1");
            }
            self.value += 1;
        }
    }

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
        saw_snapshot: std::sync::atomic::AtomicBool,
    }

    impl TransactionalCommit for FailingSnapshotRepo {
        async fn commit_batch<'a>(&'a self, batch: CommitBatch<'a>) -> Result<(), RepositoryError> {
            {
                if !batch.snapshots.is_empty() {
                    self.saw_snapshot
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                    return Err(RepositoryError::Model("snapshot write failed".into()));
                }

                for stream in batch.streams {
                    stream.entity.mark_committed();
                }
                Ok(())
            }
        }
    }

    #[tokio::test]
    async fn snapshot_batch_failure_leaves_aggregate_uncommitted() {
        let repo = FailingSnapshotRepo::default();
        let aggregate_repo = AggregateRepository::new(repo);
        let snapshot_repo = SnapshotAggregateRepository::new(aggregate_repo, 1);

        let mut aggregate = TestAggregate::default();
        aggregate.touch().unwrap();

        let err = snapshot_repo.commit(&mut aggregate).await.unwrap_err();

        assert_eq!(err, RepositoryError::Model("snapshot write failed".into()));
        assert!(snapshot_repo
            .repo()
            .repo()
            .saw_snapshot
            .load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(aggregate.entity.committed_version(), 0);
        assert_eq!(aggregate.entity.snapshot_version(), 0);
        assert_eq!(aggregate.entity.new_events().len(), 1);
    }

    #[test]
    fn snapshot_due_uses_saturating_version_distance() {
        assert!(snapshot_due(5, 2, 3));
        assert!(!snapshot_due(5, 3, 3));
        assert!(!snapshot_due(0, u64::MAX, 1));
        assert!(snapshot_due(u64::MAX, u64::MAX - 1, 1));
    }

    #[test]
    fn hydrate_from_snapshot_rejects_identity_mismatch() {
        let mut entity = Entity::with_id("snap-1");
        entity.load_from_history(vec![EventRecord::new("Touched", vec![], 1)]);
        let snapshot = SnapshotRecord::new(
            TestAggregate::aggregate_type(),
            "other",
            1,
            std::any::type_name::<u32>(),
            1,
            bitcode::serialize(&1_u32).unwrap(),
        );

        let err = match hydrate_from_snapshot::<TestAggregate>(entity, snapshot) {
            Err(err) => err,
            Ok(_) => panic!("expected identity mismatch error"),
        };

        assert!(
            matches!(err, RepositoryError::Replay(message) if message.contains("does not match"))
        );
    }

    #[test]
    fn hydrate_from_snapshot_rejects_snapshot_ahead_of_stream() {
        let mut entity = Entity::with_id("snap-1");
        entity.load_from_history(vec![EventRecord::new("Touched", vec![], 1)]);
        let snapshot = SnapshotRecord::new(
            TestAggregate::aggregate_type(),
            "snap-1",
            2,
            std::any::type_name::<u32>(),
            1,
            bitcode::serialize(&1_u32).unwrap(),
        );

        let err = match hydrate_from_snapshot::<TestAggregate>(entity, snapshot) {
            Err(err) => err,
            Ok(_) => panic!("expected future snapshot error"),
        };

        assert!(
            matches!(err, RepositoryError::Replay(message) if message.contains("exceeds stream version"))
        );
    }
}
