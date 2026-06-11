use std::future::Future;
use std::pin::Pin;

use crate::aggregate::{hydrate, AggregateRepository, SnapshotPolicy};
use crate::entity::{upcast_events, Entity};
use crate::repository::{RepositoryError, SnapshotStore, StreamIdentity};

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

impl<R, A> AggregateRepository<R, A>
where
    R: SnapshotStore + Sync,
    A: Snapshottable + Send,
{
    /// Enable snapshot caching at the given event frequency.
    ///
    /// Snapshots are a transparent optimization: this configures snapshot
    /// behaviour on the **same** repository type and returns it. On commit a
    /// snapshot is staged (when due) in the same transaction; on load the
    /// aggregate is hydrated from a snapshot when one exists. Every other method
    /// behaves identically with or without snapshots.
    pub fn with_snapshots(mut self, frequency: u64) -> Self {
        assert!(
            frequency > 0,
            "snapshot frequency must be greater than zero; \
             frequency 0 would snapshot on every commit"
        );
        self.set_snapshot_policy(SnapshotPolicy::new(
            frequency,
            snapshot_record_if_due::<A>,
            hydrate_from_store::<R, A>,
        ));
        self
    }
}

/// Build a snapshot cache record for `aggregate` when one is due at `frequency`.
/// Captured as the `record` hook of a `SnapshotPolicy`.
fn snapshot_record_if_due<A: Snapshottable>(
    aggregate: &A,
    frequency: u64,
) -> Result<Option<SnapshotRecord>, RepositoryError> {
    let version = aggregate.entity().version();
    let snap_version = aggregate.entity().snapshot_version();
    if snapshot_due(version, snap_version, frequency) {
        snapshot_record_for(aggregate).map(Some)
    } else {
        Ok(None)
    }
}

/// Load the snapshot cache record (if any) and hydrate `entity` from it.
/// Captured as the `hydrate` hook of a `SnapshotPolicy`.
fn hydrate_from_store<'a, R, A>(
    repo: &'a R,
    identity: &'a StreamIdentity,
    entity: Entity,
) -> Pin<Box<dyn Future<Output = Result<A, RepositoryError>> + Send + 'a>>
where
    R: SnapshotStore + Sync,
    A: Snapshottable + Send,
{
    Box::pin(async move {
        let snapshot = repo.get_snapshot(identity).await?;
        hydrate_with_optional_snapshot::<A>(entity, snapshot)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::{CommitBatch, TransactionalCommit};
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

    // Snapshots can only be enabled on a repo that can store them; this stub
    // satisfies the bound (the commit-failure test never loads a snapshot).
    impl SnapshotStore for FailingSnapshotRepo {
        async fn get_snapshot(
            &self,
            _identity: &StreamIdentity,
        ) -> Result<Option<SnapshotRecord>, RepositoryError> {
            Ok(None)
        }
        async fn save_snapshot(
            &self,
            _identity: &StreamIdentity,
            _record: SnapshotRecord,
        ) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn delete_snapshot(
            &self,
            _identity: &StreamIdentity,
        ) -> Result<bool, RepositoryError> {
            Ok(false)
        }
    }

    #[tokio::test]
    async fn snapshot_batch_failure_leaves_aggregate_uncommitted() {
        let repo = FailingSnapshotRepo::default();
        let snapshot_repo = AggregateRepository::new(repo).with_snapshots(1);

        let mut aggregate = TestAggregate::default();
        aggregate.touch().unwrap();

        let err = snapshot_repo.commit(&mut aggregate).await.unwrap_err();

        assert!(
            matches!(&err, RepositoryError::Model(message) if message == "snapshot write failed"),
            "unexpected error: {err}"
        );
        assert!(snapshot_repo
            .repo()
            .saw_snapshot
            .load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(aggregate.entity.committed_version(), 0);
        assert_eq!(aggregate.entity.snapshot_version(), 0);
        assert_eq!(aggregate.entity.new_events().len(), 1);
    }

    #[test]
    #[should_panic(expected = "snapshot frequency must be greater than zero")]
    fn with_snapshots_rejects_zero_frequency() {
        let _: AggregateRepository<_, TestAggregate> =
            AggregateRepository::new(FailingSnapshotRepo::default()).with_snapshots(0);
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
