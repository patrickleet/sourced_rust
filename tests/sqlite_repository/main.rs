#![cfg(feature = "sqlite")]

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use sourced_rust::{
    sourced, Aggregate, AsyncAggregateBuilder, AsyncCommitBatch, AsyncGetStream, AsyncOutboxStore,
    AsyncReadModelWritePlanCommitExt, AsyncSnapshotStore, AsyncStreamWrite,
    AsyncTransactionalCommit, Entity, OutboxMessageStatus, ReadModel, ReadModelWritePlanBuilder,
    RepositoryError, RowKey, RowPatch, RowValue, SnapshotRecord, SqliteRepository, StreamIdentity,
    TableSchemaRegistry, OUTBOX_MESSAGES_TABLE,
};

#[derive(Default)]
struct Counter {
    entity: Entity,
    value: i32,
}

#[sourced(entity, aggregate_type = "sqlite.counter")]
impl Counter {
    #[event("Incremented")]
    fn increment(&mut self, id: String, by: i32) {
        self.entity.set_id(&id);
        self.value += by;
    }
}

#[derive(Default)]
struct CounterProjection {
    entity: Entity,
}

#[sourced(entity, aggregate_type = "sqlite.counter_projection")]
impl CounterProjection {
    #[event("Touched")]
    fn touch(&mut self, id: String) {
        self.entity.set_id(&id);
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ReadModel)]
#[table("local_relational_counter_views")]
struct RelationalCounterView {
    #[id]
    id: String,
    value: i64,
    #[readmodel(jsonb)]
    counts: HashMap<String, i64>,
}

async fn repository() -> SqliteRepository {
    SqliteRepository::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap()
}

async fn bootstrap_relational_counter_table(repo: &SqliteRepository) {
    let mut registry = TableSchemaRegistry::new();
    registry.register::<RelationalCounterView>().unwrap();
    repo.bootstrap_table_schema_for_dev(&registry)
        .await
        .unwrap();
}

fn relational_counter_key(id: &str) -> RowKey {
    RowKey::new([("id", RowValue::String(id.into()))])
}

#[tokio::test]
async fn migration_is_idempotent_and_aggregate_stream_round_trips() {
    let repo = repository().await;
    repo.migrate().await.unwrap();
    let counter_repo = repo.clone().async_aggregate::<Counter>();

    let mut counter = Counter::default();
    counter.entity.set_correlation_id("corr-1");
    counter.increment("counter-1".into(), 2).unwrap();
    counter.increment("counter-1".into(), 3).unwrap();

    counter_repo.commit(&mut counter).await.unwrap();

    let loaded = counter_repo.get("counter-1").await.unwrap().unwrap();
    assert_eq!(loaded.value, 5);
    assert_eq!(loaded.entity().events().len(), 2);
    assert_eq!(loaded.entity().events()[0].sequence, 1);
    assert_eq!(loaded.entity().events()[1].sequence, 2);
    assert_eq!(loaded.entity().events()[0].correlation_id(), Some("corr-1"));
}

#[tokio::test]
async fn dev_bootstrap_applies_registered_table_schemas() {
    let repo = SqliteRepository::connect("sqlite::memory:").await.unwrap();
    let mut registry = TableSchemaRegistry::new();
    registry
        .register_schema(sourced_rust::outbox_message_schema())
        .unwrap();

    let artifacts = repo.generate_table_migration_artifacts(&registry).unwrap();
    assert!(artifacts[0]
        .statements
        .iter()
        .any(|statement| statement.contains("CREATE TABLE IF NOT EXISTS \"outbox_messages\"")));

    let bootstrap = repo
        .bootstrap_table_schema_for_dev(&registry)
        .await
        .unwrap();
    assert_eq!(
        bootstrap.bootstrapped_tables,
        vec![OUTBOX_MESSAGES_TABLE.to_string()]
    );

    let row = sqlx::query("SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?")
        .bind(OUTBOX_MESSAGES_TABLE)
        .fetch_one(repo.pool())
        .await
        .unwrap();
    let table_name: String = sqlx::Row::try_get(&row, "name").unwrap();
    assert_eq!(table_name, OUTBOX_MESSAGES_TABLE);
}

#[tokio::test]
async fn aggregate_stream_identity_separates_same_id_across_types() {
    let repo = repository().await;
    let counter_repo = repo.clone().async_aggregate::<Counter>();
    let projection_repo = repo.clone().async_aggregate::<CounterProjection>();

    let mut counter = Counter::default();
    counter.increment("shared-id".into(), 7).unwrap();
    let mut projection = CounterProjection::default();
    projection.touch("shared-id".into()).unwrap();

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
    bootstrap_relational_counter_table(&repo).await;
    let counter_repo = repo.clone().async_aggregate::<Counter>();

    let mut original = Counter::default();
    original.increment("conflict-1".into(), 1).unwrap();
    counter_repo.commit(&mut original).await.unwrap();

    let mut stale = counter_repo.get("conflict-1").await.unwrap().unwrap();
    let mut winner = counter_repo.get("conflict-1").await.unwrap().unwrap();
    stale.increment("conflict-1".into(), 10).unwrap();
    winner.increment("conflict-1".into(), 20).unwrap();
    counter_repo.commit(&mut winner).await.unwrap();

    let mut other = CounterProjection::default();
    other.touch("should-not-commit".into()).unwrap();

    let view = RelationalCounterView {
        id: "should-not-commit".into(),
        value: 99,
        counts: HashMap::new(),
    };
    let mut read_models = ReadModelWritePlanBuilder::new();
    read_models.upsert(&view).unwrap();

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
    let remaining: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM "local_relational_counter_views"
        WHERE "id" = ?
        "#,
    )
    .bind("should-not-commit")
    .fetch_one(repo.pool())
    .await
    .unwrap();
    assert_eq!(remaining, 0);
    assert_eq!(stale.entity().committed_version(), 1);
    assert_eq!(stale.entity().new_events().len(), 1);
}

#[tokio::test]
async fn commit_batch_lowers_relational_read_model_plan_into_registered_table() {
    let repo = repository().await;
    bootstrap_relational_counter_table(&repo).await;
    let mut counts = HashMap::new();
    counts.insert("wins".to_string(), 2);
    let view = RelationalCounterView {
        id: "relational-batch-1".into(),
        value: 7,
        counts,
    };
    let mut session = ReadModelWritePlanBuilder::new();
    session.upsert(&view).unwrap();
    let mut projection = CounterProjection::default();
    projection.touch("relational-batch-1".into()).unwrap();
    let identity =
        StreamIdentity::new(CounterProjection::aggregate_type(), "relational-batch-1").unwrap();

    repo.read_models(session)
        .commit(&mut projection)
        .await
        .unwrap();

    assert!(repo.get_stream(&identity).await.unwrap().is_some());
    let row = sqlx::query(
        r#"
        SELECT "id", "value", "counts", "_sourced_version"
        FROM "local_relational_counter_views"
        WHERE "id" = ?
        "#,
    )
    .bind("relational-batch-1")
    .fetch_one(repo.pool())
    .await
    .unwrap();
    let stored_counts: String = sqlx::Row::try_get(&row, "counts").unwrap();
    let stored_counts: serde_json::Value = serde_json::from_str(&stored_counts).unwrap();

    assert_eq!(
        sqlx::Row::try_get::<String, _>(&row, "id").unwrap(),
        "relational-batch-1"
    );
    assert_eq!(sqlx::Row::try_get::<i64, _>(&row, "value").unwrap(), 7);
    assert_eq!(stored_counts["wins"].as_i64(), Some(2));
    assert_eq!(
        sqlx::Row::try_get::<i64, _>(&row, "_sourced_version").unwrap(),
        1
    );
}

#[tokio::test]
async fn read_model_session_patches_and_deletes_relational_rows() {
    let repo = repository().await;
    bootstrap_relational_counter_table(&repo).await;
    let mut counts = HashMap::new();
    counts.insert("wins".to_string(), 2);
    let view = RelationalCounterView {
        id: "relational-session-1".into(),
        value: 7,
        counts,
    };
    let mut setup = ReadModelWritePlanBuilder::new();
    setup.upsert(&view).unwrap();
    setup.commit_async(&repo).await.unwrap();

    let mut patched_counts = HashMap::new();
    patched_counts.insert("wins".to_string(), 3);
    patched_counts.insert("losses".to_string(), 1);
    let patch = RowPatch::new()
        .set("value", RowValue::I64(11))
        .set_serde("counts", &patched_counts)
        .unwrap();
    let mut patch_session = ReadModelWritePlanBuilder::new();
    patch_session
        .patch::<RelationalCounterView>(relational_counter_key("relational-session-1"), patch)
        .unwrap();
    patch_session.commit_async(&repo).await.unwrap();

    let row = sqlx::query(
        r#"
        SELECT "value", "counts", "_sourced_version"
        FROM "local_relational_counter_views"
        WHERE "id" = ?
        "#,
    )
    .bind("relational-session-1")
    .fetch_one(repo.pool())
    .await
    .unwrap();
    let stored_counts: String = sqlx::Row::try_get(&row, "counts").unwrap();
    let stored_counts: serde_json::Value = serde_json::from_str(&stored_counts).unwrap();
    assert_eq!(sqlx::Row::try_get::<i64, _>(&row, "value").unwrap(), 11);
    assert_eq!(stored_counts["wins"].as_i64(), Some(3));
    assert_eq!(stored_counts["losses"].as_i64(), Some(1));
    assert_eq!(
        sqlx::Row::try_get::<i64, _>(&row, "_sourced_version").unwrap(),
        2
    );

    let mut delete_session = ReadModelWritePlanBuilder::new();
    delete_session
        .delete::<RelationalCounterView>(relational_counter_key("relational-session-1"))
        .unwrap();
    delete_session.commit_async(&repo).await.unwrap();

    let remaining: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM "local_relational_counter_views"
        WHERE "id" = ?
        "#,
    )
    .bind("relational-session-1")
    .fetch_one(repo.pool())
    .await
    .unwrap();
    assert_eq!(remaining, 0);
}

#[tokio::test]
async fn read_model_session_persists_relational_rows() {
    let repo = repository().await;
    bootstrap_relational_counter_table(&repo).await;
    let view = RelationalCounterView {
        id: "view-1".into(),
        value: 42,
        counts: HashMap::new(),
    };
    let mut session = ReadModelWritePlanBuilder::new();
    session.upsert(&view).unwrap();

    let outcome = session.commit_async(&repo).await.unwrap();
    let row = sqlx::query(
        r#"
        SELECT "value", "_sourced_version"
        FROM "local_relational_counter_views"
        WHERE "id" = ?
        "#,
    )
    .bind("view-1")
    .fetch_one(repo.pool())
    .await
    .unwrap();

    assert!(outcome.was_applied());
    assert_eq!(sqlx::Row::try_get::<i64, _>(&row, "value").unwrap(), 42);
    assert_eq!(
        sqlx::Row::try_get::<i64, _>(&row, "_sourced_version").unwrap(),
        1
    );
}

#[tokio::test]
async fn snapshots_persist_by_full_stream_identity() {
    let repo = repository().await;
    let counter = StreamIdentity::new("sqlite.counter", "same-id").unwrap();
    let projection = StreamIdentity::new("sqlite.counter_projection", "same-id").unwrap();

    repo.save_snapshot_async(
        &counter,
        SnapshotRecord::new(
            "sqlite.counter",
            "same-id",
            1,
            "CounterSnapshot",
            1,
            vec![1],
        ),
    )
    .await
    .unwrap();
    repo.save_snapshot_async(
        &projection,
        SnapshotRecord::new(
            "sqlite.counter_projection",
            "same-id",
            2,
            "ProjectionSnapshot",
            1,
            vec![2],
        ),
    )
    .await
    .unwrap();

    let loaded_counter = repo.get_snapshot_async(&counter).await.unwrap().unwrap();
    let loaded_projection = repo.get_snapshot_async(&projection).await.unwrap().unwrap();

    assert_eq!(loaded_counter.version, 1);
    assert_eq!(loaded_counter.aggregate_type, "sqlite.counter");
    assert_eq!(loaded_counter.snapshot_type, "CounterSnapshot");
    assert_eq!(loaded_counter.payload, vec![1]);
    assert_eq!(loaded_projection.version, 2);
    assert_eq!(
        loaded_projection.aggregate_type,
        "sqlite.counter_projection"
    );
    assert_eq!(loaded_projection.snapshot_type, "ProjectionSnapshot");
    assert_eq!(loaded_projection.payload, vec![2]);
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

#[tokio::test]
async fn outbox_metadata_columns_round_trip_into_message_metadata() {
    let repo = repository().await;
    let message_id = "outbox-column-metadata";
    sqlx::query(
        r#"
        INSERT INTO outbox_messages (
          message_id,
          event_type,
          payload,
          payload_codec,
          payload_codec_version,
          metadata,
          status,
          created_at,
          next_available_at,
          attempts,
          correlation_id,
          causation_id
        )
        VALUES (?, 'OutboxColumns', x'00', 'bytes', 1, '{}', 'pending',
                '0.000000000', '0.000000000', 0, 'corr-column', 'cause-column')
        "#,
    )
    .bind(message_id)
    .execute(repo.pool())
    .await
    .unwrap();

    let stored = repo
        .outbox_store()
        .messages_by_status_async(OutboxMessageStatus::Pending)
        .await
        .unwrap()
        .into_iter()
        .find(|message| message.id() == message_id)
        .unwrap();

    assert_eq!(stored.correlation_id(), Some("corr-column"));
    assert_eq!(stored.causation_id(), Some("cause-column"));
}
