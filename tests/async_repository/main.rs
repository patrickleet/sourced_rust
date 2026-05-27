use std::time::Duration;

use serde::{Deserialize, Serialize};
use sourced_rust::{
    impl_aggregate, Aggregate, AsyncAggregateBuilder, AsyncCommitBatch, AsyncGetStream,
    AsyncOutboxStore, AsyncReadModelStore, AsyncReadModelWritePlanStore, AsyncSnapshotStore,
    AsyncStreamWrite, AsyncTransactionalCommit, ClaimOutboxMessages, Entity, EventRecord,
    HashMapRepository, InMemorySnapshotStore, OutboxMessage, ProcessedMessageMark, ReadModel,
    ReadModelWritePlan, ReadModelWritePlanBuilder, RepositoryError, SnapshotRecord, Snapshottable,
    StreamIdentity,
};

#[derive(Default)]
struct AlphaAggregate {
    entity: Entity,
}

impl AlphaAggregate {
    fn touch(&mut self, id: &str) {
        self.entity.set_id(id);
        self.entity.digest_empty("Touched").unwrap();
    }

    fn replay(&mut self, _event: &EventRecord) -> Result<(), String> {
        Ok(())
    }
}

impl_aggregate!(
    AlphaAggregate,
    entity,
    replay,
    aggregate_type = "async.alpha"
);

#[derive(Default)]
struct BetaAggregate {
    entity: Entity,
}

impl BetaAggregate {
    fn touch(&mut self, id: &str) {
        self.entity.set_id(id);
        self.entity.digest_empty("Touched").unwrap();
    }

    fn replay(&mut self, _event: &EventRecord) -> Result<(), String> {
        Ok(())
    }
}

impl_aggregate!(BetaAggregate, entity, replay, aggregate_type = "async.beta");

#[derive(Default)]
struct SnapshotCounter {
    entity: Entity,
    value: i32,
}

impl SnapshotCounter {
    fn increment(&mut self, id: &str, by: i32) {
        self.entity.set_id(id);
        self.entity.digest("Incremented", &by).unwrap();
        self.value += by;
    }

    fn replay(&mut self, event: &EventRecord) -> Result<(), String> {
        if event.event_name == "Incremented" {
            self.value += event.decode::<i32>().map_err(|err| err.to_string())?;
        }
        Ok(())
    }
}

impl_aggregate!(
    SnapshotCounter,
    entity,
    replay,
    aggregate_type = "async.snapshot_counter"
);

impl Snapshottable for SnapshotCounter {
    type Snapshot = i32;

    fn create_snapshot(&self) -> Self::Snapshot {
        self.value
    }

    fn restore_from_snapshot(&mut self, snapshot: Self::Snapshot) {
        self.value = snapshot;
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct TestView {
    id: String,
    value: i32,
}

impl ReadModel for TestView {
    const COLLECTION: &'static str = "async_test_views";

    fn id(&self) -> &str {
        &self.id
    }
}

#[tokio::test]
async fn async_aggregate_repository_separates_streams_by_aggregate_type() {
    let repo = HashMapRepository::new();
    let alpha_repo = repo.clone().async_aggregate::<AlphaAggregate>();
    let beta_repo = repo.clone().async_aggregate::<BetaAggregate>();

    let mut alpha = AlphaAggregate::default();
    alpha.touch("shared-id");
    let mut beta = BetaAggregate::default();
    beta.touch("shared-id");

    alpha_repo.commit(&mut alpha).await.unwrap();
    beta_repo.commit(&mut beta).await.unwrap();

    let loaded_alpha = alpha_repo.get("shared-id").await.unwrap().unwrap();
    let loaded_beta = beta_repo.get("shared-id").await.unwrap().unwrap();

    assert_eq!(loaded_alpha.entity().events().len(), 1);
    assert_eq!(loaded_beta.entity().events().len(), 1);
}

#[tokio::test]
async fn async_batch_rejects_duplicate_stream_identity_before_write() {
    let repo = HashMapRepository::new();
    let identity = StreamIdentity::new("async.alpha", "duplicate").unwrap();
    let mut first = Entity::with_id("duplicate");
    first.digest_empty("First").unwrap();
    let mut second = Entity::with_id("duplicate");
    second.digest_empty("Second").unwrap();

    let err = repo
        .commit_batch_async(AsyncCommitBatch::new(vec![
            AsyncStreamWrite::new(identity.clone(), &mut first),
            AsyncStreamWrite::new(identity.clone(), &mut second),
        ]))
        .await
        .unwrap_err();

    assert_eq!(
        err,
        RepositoryError::DuplicateStreamInBatch {
            id: "async.alpha:duplicate".into()
        }
    );
    assert!(repo.get_stream(&identity).await.unwrap().is_none());
}

#[tokio::test]
async fn async_batch_read_model_failure_rolls_back_stream_append() {
    let repo = HashMapRepository::new();
    let identity = StreamIdentity::new("async.rollback", "rollback-1").unwrap();
    let mut entity = Entity::with_id("rollback-1");
    entity.digest_empty("Touched").unwrap();
    let mark = ProcessedMessageMark {
        consumer_name: "projection".into(),
        message_id: "event-1".into(),
    };
    let plan = ReadModelWritePlan::new(Vec::new(), vec![mark.clone(), mark]);

    let err = repo
        .commit_batch_async(AsyncCommitBatch {
            streams: vec![AsyncStreamWrite::new(identity.clone(), &mut entity)],
            outbox_messages: Vec::new(),
            read_model_plans: vec![plan],
            snapshots: Vec::new(),
        })
        .await
        .unwrap_err();

    assert!(
        matches!(err, RepositoryError::Model(message) if message.contains("processed message already handled"))
    );
    assert!(repo.get_stream(&identity).await.unwrap().is_none());
    assert_eq!(entity.committed_version(), 0);
    assert_eq!(entity.new_events().len(), 1);
}

#[tokio::test]
async fn read_model_session_can_commit_against_async_store() {
    let repo = HashMapRepository::new();
    let view = TestView {
        id: "view-1".into(),
        value: 42,
    };
    let mut session = ReadModelWritePlanBuilder::new();
    session
        .document(&view)
        .unwrap()
        .mark_processed("projection", "event-1");

    let outcome = session.commit_async(&repo).await.unwrap();
    let loaded = repo
        .get_model_async::<TestView>("view-1")
        .await
        .unwrap()
        .unwrap();
    let processed = repo
        .is_processed_async("projection", "event-1")
        .await
        .unwrap();

    assert!(outcome.was_applied());
    assert_eq!(loaded.data, view);
    assert!(processed);
}

#[tokio::test]
async fn async_snapshot_store_uses_full_stream_identity() {
    let store = InMemorySnapshotStore::new();
    let alpha = StreamIdentity::new("async.alpha", "same-id").unwrap();
    let beta = StreamIdentity::new("async.beta", "same-id").unwrap();

    store
        .save_snapshot_async(
            &alpha,
            SnapshotRecord::new("async.alpha", "same-id", 1, "AlphaSnapshot", 1, vec![1]),
        )
        .await
        .unwrap();
    store
        .save_snapshot_async(
            &beta,
            SnapshotRecord::new("async.beta", "same-id", 2, "BetaSnapshot", 1, vec![2]),
        )
        .await
        .unwrap();

    let loaded_alpha = store.get_snapshot_async(&alpha).await.unwrap().unwrap();
    let loaded_beta = store.get_snapshot_async(&beta).await.unwrap().unwrap();

    assert_eq!(loaded_alpha.version, 1);
    assert_eq!(loaded_beta.version, 2);
    assert_eq!(loaded_alpha.aggregate_type, "async.alpha");
    assert_eq!(loaded_beta.aggregate_type, "async.beta");
}

#[tokio::test]
async fn async_snapshot_repository_writes_cache_without_event_record() {
    let repo = HashMapRepository::new();
    let snapshot_repo = repo
        .clone()
        .async_aggregate::<SnapshotCounter>()
        .with_snapshots(2);
    let id = "snapshot-counter-1";

    let mut counter = SnapshotCounter::default();
    counter.increment(id, 2);
    snapshot_repo.commit(&mut counter).await.unwrap();
    counter.increment(id, 3);
    snapshot_repo.commit(&mut counter).await.unwrap();

    let identity = StreamIdentity::new(SnapshotCounter::aggregate_type(), id).unwrap();
    let stream = repo.get_stream(&identity).await.unwrap().unwrap();
    let snapshot = repo.get_snapshot_async(&identity).await.unwrap().unwrap();

    assert_eq!(stream.events().len(), 2);
    assert_eq!(stream.events()[0].event_name, "Incremented");
    assert_eq!(stream.events()[1].event_name, "Incremented");
    assert_eq!(snapshot.version, 2);
    assert_eq!(snapshot.aggregate_type, SnapshotCounter::aggregate_type());
    assert_eq!(snapshot.payload, bitcode::serialize(&5_i32).unwrap());
}

#[tokio::test]
async fn async_snapshot_repository_ignores_invalid_cache_and_replays_events() {
    let repo = HashMapRepository::new();
    let aggregate_repo = repo.clone().async_aggregate::<SnapshotCounter>();
    let snapshot_repo = repo
        .clone()
        .async_aggregate::<SnapshotCounter>()
        .with_snapshots(10);
    let id = "snapshot-counter-invalid";

    let mut counter = SnapshotCounter::default();
    counter.increment(id, 4);
    counter.increment(id, 6);
    aggregate_repo.commit(&mut counter).await.unwrap();

    let identity = StreamIdentity::new(SnapshotCounter::aggregate_type(), id).unwrap();
    let mut invalid = SnapshotRecord::new(
        SnapshotCounter::aggregate_type(),
        id,
        1,
        std::any::type_name::<i32>(),
        1,
        vec![0xff],
    );
    invalid.payload_codec = "json".into();
    repo.save_snapshot_async(&identity, invalid).await.unwrap();

    let loaded = snapshot_repo.get(id).await.unwrap().unwrap();
    assert_eq!(loaded.value, 10);
    assert_eq!(loaded.entity().snapshot_version(), 0);
}

#[tokio::test]
async fn async_snapshot_repository_ignores_cache_past_stream_version_and_replays_events() {
    let repo = HashMapRepository::new();
    let aggregate_repo = repo.clone().async_aggregate::<SnapshotCounter>();
    let snapshot_repo = repo
        .clone()
        .async_aggregate::<SnapshotCounter>()
        .with_snapshots(10);
    let id = "snapshot-counter-ahead";

    let mut counter = SnapshotCounter::default();
    counter.increment(id, 4);
    aggregate_repo.commit(&mut counter).await.unwrap();

    let identity = StreamIdentity::new(SnapshotCounter::aggregate_type(), id).unwrap();
    let record = SnapshotRecord::new(
        SnapshotCounter::aggregate_type(),
        id,
        2,
        std::any::type_name::<i32>(),
        1,
        bitcode::serialize(&999_i32).unwrap(),
    );
    repo.save_snapshot_async(&identity, record).await.unwrap();

    let loaded = snapshot_repo.get(id).await.unwrap().unwrap();
    assert_eq!(loaded.value, 4);
    assert_eq!(loaded.entity().version(), 1);
    assert_eq!(loaded.entity().snapshot_version(), 0);
}

#[tokio::test]
async fn async_outbox_repository_delegates_worker_operations() {
    let repo = HashMapRepository::new();
    let outbox = repo.outbox_store();
    let message = OutboxMessage::create("msg-1", "Event", b"{}".to_vec()).unwrap();
    let mut aggregate = AlphaAggregate::default();
    aggregate.touch("outbox-aggregate-1");
    repo.clone()
        .async_aggregate::<AlphaAggregate>()
        .outbox(message)
        .commit(&mut aggregate)
        .await
        .unwrap();

    let claimed = outbox
        .claim_async(ClaimOutboxMessages::new(
            "worker-1",
            1,
            Duration::from_secs(60),
        ))
        .await
        .unwrap();

    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].worker_id.as_deref(), Some("worker-1"));
}
