use std::time::Duration;

use serde::{Deserialize, Serialize};
use sourced_rust::{
    impl_aggregate, Aggregate, AsyncAggregateBuilder, AsyncCommitBatch, AsyncGetStream,
    AsyncOutboxStore, AsyncReadModelSessionStore, AsyncReadModelStore, AsyncSnapshotStore,
    AsyncStreamWrite, AsyncTransactionalCommit, ClaimOutboxMessages, Entity, EventRecord,
    HashMapRepository, InMemorySnapshotStore, OutboxMessage, ProcessedMessageMark, ReadModel,
    ReadModelSession, ReadModelWritePlan, RepositoryError, SnapshotRecord, StreamIdentity,
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
    let mut session = ReadModelSession::new();
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
            SnapshotRecord {
                aggregate_id: "same-id".into(),
                version: 1,
                data: vec![1],
            },
        )
        .await
        .unwrap();
    store
        .save_snapshot_async(
            &beta,
            SnapshotRecord {
                aggregate_id: "same-id".into(),
                version: 2,
                data: vec![2],
            },
        )
        .await
        .unwrap();

    let loaded_alpha = store.get_snapshot_async(&alpha).await.unwrap().unwrap();
    let loaded_beta = store.get_snapshot_async(&beta).await.unwrap().unwrap();

    assert_eq!(loaded_alpha.version, 1);
    assert_eq!(loaded_beta.version, 2);
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
