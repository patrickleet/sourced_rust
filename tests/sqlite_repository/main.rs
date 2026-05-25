#![cfg(feature = "sqlite")]

use serde::{Deserialize, Serialize};
use sourced_rust::{
    impl_aggregate, Aggregate, AsyncAggregateBuilder, AsyncCommitBatch, AsyncGetStream,
    AsyncReadModelSessionStore, AsyncReadModelStore, AsyncSnapshotStore, AsyncStreamWrite,
    AsyncTransactionalCommit, Entity, EventRecord, ReadModel, ReadModelSession, RepositoryError,
    SnapshotRecord, SqliteRepository, StreamIdentity,
};

#[derive(Default)]
struct Counter {
    entity: Entity,
    value: i32,
}

impl Counter {
    fn increment(&mut self, id: &str, by: i32) {
        self.entity.set_id(id);
        self.entity.digest("Incremented", &by).unwrap();
        self.value += by;
    }

    fn replay(&mut self, event: &EventRecord) -> Result<(), String> {
        if event.event_name == "Incremented" {
            let by = event.decode::<i32>().map_err(|err| err.to_string())?;
            self.value += by;
        }
        Ok(())
    }
}

impl_aggregate!(Counter, entity, replay, aggregate_type = "sqlite.counter");

#[derive(Default)]
struct CounterProjection {
    entity: Entity,
}

impl CounterProjection {
    fn touch(&mut self, id: &str) {
        self.entity.set_id(id);
        self.entity.digest_empty("Touched").unwrap();
    }

    fn replay(&mut self, _event: &EventRecord) -> Result<(), String> {
        Ok(())
    }
}

impl_aggregate!(
    CounterProjection,
    entity,
    replay,
    aggregate_type = "sqlite.counter_projection"
);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct CounterView {
    id: String,
    value: i32,
}

impl ReadModel for CounterView {
    const COLLECTION: &'static str = "sqlite_counter_views";

    fn id(&self) -> &str {
        &self.id
    }
}

async fn repository() -> SqliteRepository {
    SqliteRepository::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap()
}

#[tokio::test]
async fn migration_is_idempotent_and_aggregate_stream_round_trips() {
    let repo = repository().await;
    repo.migrate().await.unwrap();
    let counter_repo = repo.clone().async_aggregate::<Counter>();

    let mut counter = Counter::default();
    counter.entity.set_correlation_id("corr-1");
    counter.increment("counter-1", 2);
    counter.increment("counter-1", 3);

    counter_repo.commit(&mut counter).await.unwrap();

    let loaded = counter_repo.get("counter-1").await.unwrap().unwrap();
    assert_eq!(loaded.value, 5);
    assert_eq!(loaded.entity().events().len(), 2);
    assert_eq!(loaded.entity().events()[0].sequence, 1);
    assert_eq!(loaded.entity().events()[1].sequence, 2);
    assert_eq!(loaded.entity().events()[0].correlation_id(), Some("corr-1"));
}

#[tokio::test]
async fn aggregate_stream_identity_separates_same_id_across_types() {
    let repo = repository().await;
    let counter_repo = repo.clone().async_aggregate::<Counter>();
    let projection_repo = repo.clone().async_aggregate::<CounterProjection>();

    let mut counter = Counter::default();
    counter.increment("shared-id", 7);
    let mut projection = CounterProjection::default();
    projection.touch("shared-id");

    counter_repo.commit(&mut counter).await.unwrap();
    projection_repo.commit(&mut projection).await.unwrap();

    let loaded_counter = counter_repo.get("shared-id").await.unwrap().unwrap();
    let loaded_projection = projection_repo.get("shared-id").await.unwrap().unwrap();

    assert_eq!(loaded_counter.value, 7);
    assert_eq!(loaded_counter.entity().events().len(), 1);
    assert_eq!(loaded_projection.entity().events().len(), 1);
}

#[tokio::test]
async fn optimistic_conflict_rolls_back_other_stream_and_read_model_plan() {
    let repo = repository().await;
    let counter_repo = repo.clone().async_aggregate::<Counter>();

    let mut original = Counter::default();
    original.increment("conflict-1", 1);
    counter_repo.commit(&mut original).await.unwrap();

    let mut stale = counter_repo.get("conflict-1").await.unwrap().unwrap();
    let mut winner = counter_repo.get("conflict-1").await.unwrap().unwrap();
    stale.increment("conflict-1", 10);
    winner.increment("conflict-1", 20);
    counter_repo.commit(&mut winner).await.unwrap();

    let mut other = CounterProjection::default();
    other.touch("should-not-commit");

    let view = CounterView {
        id: "should-not-commit".into(),
        value: 99,
    };
    let mut read_models = ReadModelSession::new();
    read_models.document(&view).unwrap();

    let stale_identity = StreamIdentity::new(Counter::aggregate_type(), "conflict-1").unwrap();
    let other_identity =
        StreamIdentity::new(CounterProjection::aggregate_type(), "should-not-commit").unwrap();
    let err = repo
        .commit_batch_async(AsyncCommitBatch {
            streams: vec![
                AsyncStreamWrite::new(stale_identity.clone(), stale.entity_mut()),
                AsyncStreamWrite::new(other_identity.clone(), other.entity_mut()),
            ],
            outbox_messages: Vec::new(),
            read_model_plans: vec![read_models.into_write_plan().unwrap()],
            snapshots: Vec::new(),
        })
        .await
        .unwrap_err();

    assert!(matches!(err, RepositoryError::ConcurrentWrite { .. }));
    assert!(repo.get_stream(&other_identity).await.unwrap().is_none());
    assert!(repo
        .get_model_async::<CounterView>("should-not-commit")
        .await
        .unwrap()
        .is_none());
    assert_eq!(stale.entity().committed_version(), 1);
    assert_eq!(stale.entity().new_events().len(), 1);
}

#[tokio::test]
async fn read_model_session_persists_documents_and_processed_marks() {
    let repo = repository().await;
    let view = CounterView {
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
        .get_model_async::<CounterView>("view-1")
        .await
        .unwrap()
        .unwrap();
    let processed = repo
        .is_processed_async("projection", "event-1")
        .await
        .unwrap();

    assert!(outcome.was_applied());
    assert_eq!(loaded.version, 1);
    assert_eq!(loaded.data, view);
    assert!(processed);

    let mut duplicate = ReadModelSession::new();
    duplicate
        .document(&CounterView {
            id: "view-1".into(),
            value: 100,
        })
        .unwrap()
        .mark_processed("projection", "event-1");
    let duplicate_outcome = duplicate.commit_async(&repo).await.unwrap();
    let still_loaded = repo
        .get_model_async::<CounterView>("view-1")
        .await
        .unwrap()
        .unwrap();

    assert!(duplicate_outcome.was_skipped());
    assert_eq!(still_loaded.data.value, 42);
}

#[tokio::test]
async fn snapshots_persist_by_full_stream_identity() {
    let repo = repository().await;
    let counter = StreamIdentity::new("sqlite.counter", "same-id").unwrap();
    let projection = StreamIdentity::new("sqlite.counter_projection", "same-id").unwrap();

    repo.save_snapshot_async(
        &counter,
        SnapshotRecord {
            aggregate_id: "same-id".into(),
            version: 1,
            data: vec![1],
        },
    )
    .await
    .unwrap();
    repo.save_snapshot_async(
        &projection,
        SnapshotRecord {
            aggregate_id: "same-id".into(),
            version: 2,
            data: vec![2],
        },
    )
    .await
    .unwrap();

    let loaded_counter = repo.get_snapshot_async(&counter).await.unwrap().unwrap();
    let loaded_projection = repo.get_snapshot_async(&projection).await.unwrap().unwrap();

    assert_eq!(loaded_counter.version, 1);
    assert_eq!(loaded_counter.data, vec![1]);
    assert_eq!(loaded_projection.version, 2);
    assert_eq!(loaded_projection.data, vec![2]);
}

#[tokio::test]
async fn unsupported_codec_rows_fail_on_read() {
    let repo = repository().await;
    sqlx::query(
        r#"
        INSERT INTO aggregate_events (
          aggregate_type,
          aggregate_id,
          sequence,
          event_name,
          event_version,
          payload,
          payload_codec,
          payload_codec_version,
          metadata,
          recorded_at
        )
        VALUES (?, ?, 1, 'BadEvent', 1, x'00', 'json', 1, '{}', '0.000000000')
        "#,
    )
    .bind("sqlite.counter")
    .bind("bad-codec")
    .execute(repo.pool())
    .await
    .unwrap();

    let identity = StreamIdentity::new("sqlite.counter", "bad-codec").unwrap();
    let err = repo.get_stream(&identity).await.unwrap_err();

    assert!(
        matches!(err, RepositoryError::Model(message) if message.contains("unsupported payload codec"))
    );
}
