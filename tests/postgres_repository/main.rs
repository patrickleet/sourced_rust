#![cfg(feature = "postgres")]

#[path = "../support/postgres.rs"]
mod postgres;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use distributed::{
    sourced, Aggregate, AggregateBuilder, AsyncOutboxStore, CommitBatch, Entity, GetStream,
    OutboxMessage, OutboxMessageStatus, PostgresRepository, ReadModel, ReadModelWritePlanBuilder,
    ReadModelWritePlanCommitExt, RepositoryError, RowKey, RowPatch, RowValue, SnapshotRecord,
    SnapshotStore, StreamIdentity, StreamWrite, TableSchemaRegistry, TransactionalCommit,
};
use serde::{Deserialize, Serialize};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Default)]
struct Counter {
    entity: Entity,
    value: i32,
}

#[sourced(entity, aggregate_type = "postgres.counter")]
impl Counter {
    #[event("incremented")]
    fn increment(&mut self, id: String, by: i32) {
        self.entity.set_id(&id);
        self.value += by;
    }
}

#[derive(Default)]
struct CounterProjection {
    entity: Entity,
}

#[sourced(entity, aggregate_type = "postgres.counter_projection")]
impl CounterProjection {
    #[event("touched")]
    fn touch(&mut self, id: String) {
        self.entity.set_id(&id);
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ReadModel)]
#[table("postgres_relational_counter_views")]
struct RelationalCounterView {
    #[id]
    id: String,
    value: i64,
    #[readmodel(jsonb)]
    counts: HashMap<String, i64>,
}

async fn repository() -> Option<(postgres::PostgresTestSchema, PostgresRepository)> {
    let schema = postgres::PostgresTestSchema::create_from_env(
        "postgres_repo",
        "skipping Postgres integration test",
    )
    .await?;
    let repo = schema.repository().await;
    Some((schema, repo))
}

fn unique_id(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{nanos}-{id}")
}

// Consumer inbox semantics are covered for all backends by the shared
// `persistent_repository_conformance::inbox` scenarios.

async fn bootstrap_relational_counter_table(repo: &PostgresRepository) {
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
async fn migration_is_idempotent_and_uses_postgres_column_types() {
    let Some((schema, repo)) = repository().await else {
        return;
    };
    repo.migrate().await.unwrap();

    let rows = sqlx::query(
        r#"
        SELECT column_name, udt_name
        FROM information_schema.columns
        WHERE table_schema = $1
          AND table_name = 'aggregate_events'
          AND column_name IN ('payload', 'metadata', 'recorded_at')
        "#,
    )
    .bind(schema.schema_name())
    .fetch_all(repo.pool())
    .await
    .unwrap();

    let mut columns = rows
        .into_iter()
        .map(|row| {
            (
                sqlx::Row::try_get::<String, _>(&row, "column_name").unwrap(),
                sqlx::Row::try_get::<String, _>(&row, "udt_name").unwrap(),
            )
        })
        .collect::<Vec<_>>();
    columns.sort();

    assert_eq!(
        columns,
        vec![
            ("metadata".into(), "jsonb".into()),
            ("payload".into(), "bytea".into()),
            ("recorded_at".into(), "timestamptz".into()),
        ]
    );

    let document_table: Option<String> = sqlx::query_scalar(
        r#"
        SELECT table_name
        FROM information_schema.tables
        WHERE table_schema = $1
          AND table_name = 'transactional_read_models'
        "#,
    )
    .bind(schema.schema_name())
    .fetch_optional(repo.pool())
    .await
    .unwrap();
    assert!(document_table.is_none());

    let processed_messages_table: Option<String> = sqlx::query_scalar(
        r#"
        SELECT table_name
        FROM information_schema.tables
        WHERE table_schema = $1
          AND table_name = 'read_model_processed_messages'
        "#,
    )
    .bind(schema.schema_name())
    .fetch_optional(repo.pool())
    .await
    .unwrap();
    assert!(processed_messages_table.is_none());
}

#[tokio::test]
async fn aggregate_stream_round_trips_with_metadata() {
    let Some((_schema, repo)) = repository().await else {
        return;
    };
    let counter_repo = repo.clone().aggregate::<Counter>();
    let id = unique_id("counter");

    let mut counter = Counter::default();
    counter.entity.set_correlation_id("corr-postgres");
    counter.increment(id.clone(), 2).unwrap();
    counter.increment(id.clone(), 3).unwrap();

    counter_repo.commit(&mut counter).await.unwrap();

    let loaded = counter_repo.get(&id).await.unwrap().unwrap();
    assert_eq!(loaded.value, 5);
    assert_eq!(loaded.entity().events().len(), 2);
    assert_eq!(loaded.entity().events()[0].sequence, 1);
    assert_eq!(loaded.entity().events()[1].sequence, 2);
    assert_eq!(
        loaded.entity().events()[0].correlation_id(),
        Some("corr-postgres")
    );
}

#[tokio::test]
async fn optimistic_conflict_rolls_back_other_stream_and_snapshot() {
    let Some((_schema, repo)) = repository().await else {
        return;
    };
    let counter_repo = repo.clone().aggregate::<Counter>();
    let counter_id = unique_id("conflict");
    let other_id = unique_id("rollback");

    let mut original = Counter::default();
    original.increment(counter_id.clone(), 1).unwrap();
    counter_repo.commit(&mut original).await.unwrap();

    let mut stale = counter_repo.get(&counter_id).await.unwrap().unwrap();
    let mut winner = counter_repo.get(&counter_id).await.unwrap().unwrap();
    stale.increment(counter_id.clone(), 10).unwrap();
    winner.increment(counter_id.clone(), 20).unwrap();
    counter_repo.commit(&mut winner).await.unwrap();

    let mut other = CounterProjection::default();
    other.touch(other_id.clone()).unwrap();

    let stale_identity = StreamIdentity::new(Counter::aggregate_type(), &counter_id).unwrap();
    let other_identity =
        StreamIdentity::new(CounterProjection::aggregate_type(), &other_id).unwrap();
    let err = repo
        .commit_batch(CommitBatch {
            inbox_receipts: Vec::new(),
            streams: vec![
                StreamWrite::new(stale_identity, stale.entity_mut()),
                StreamWrite::new(other_identity.clone(), other.entity_mut()),
            ],
            outbox_messages: Vec::new(),
            read_model_plans: Vec::new(),
            snapshots: vec![distributed::SnapshotWrite::Save {
                identity: other_identity.clone(),
                record: SnapshotRecord::new(
                    CounterProjection::aggregate_type(),
                    other_id.clone(),
                    1,
                    "CounterProjectionSnapshot",
                    1,
                    vec![1],
                ),
            }],
        })
        .await
        .unwrap_err();

    assert!(matches!(err, RepositoryError::ConcurrentWrite { .. }));
    assert!(repo.get_stream(&other_identity).await.unwrap().is_none());
    assert!(repo.get_snapshot(&other_identity).await.unwrap().is_none());
    assert_eq!(stale.entity().committed_version(), 1);
    assert_eq!(stale.entity().new_events().len(), 1);
}

#[tokio::test]
async fn duplicate_stream_identity_is_rejected_before_sql_writes() {
    let Some((_schema, repo)) = repository().await else {
        return;
    };
    let id = unique_id("duplicate");
    let identity = StreamIdentity::new(Counter::aggregate_type(), &id).unwrap();
    let mut first = Entity::with_id(&id);
    first.digest_empty("first_recorded").unwrap();
    let mut second = Entity::with_id(&id);
    second.digest_empty("second_recorded").unwrap();

    let err = repo
        .commit_batch(CommitBatch::new(vec![
            StreamWrite::new(identity.clone(), &mut first),
            StreamWrite::new(identity.clone(), &mut second),
        ]))
        .await
        .unwrap_err();

    assert_eq!(
        err,
        RepositoryError::DuplicateStreamInBatch {
            id: format!("{}:{id}", Counter::aggregate_type())
        }
    );
    assert!(repo.get_stream(&identity).await.unwrap().is_none());
}

#[tokio::test]
async fn read_model_failure_mid_plan_rolls_back_events_and_outbox() {
    // Postgres variant of the SQLite mid-plan rollback test: a commit carrying an
    // aggregate event, an outbox row, and a two-mutation read-model plan whose
    // second mutation violates a real CHECK constraint must roll the WHOLE
    // transaction back — event, outbox row, and the first (already-applied)
    // read-model mutation all absent. Skips locally without DATABASE_URL.
    let Some((_schema, repo)) = repository().await else {
        return;
    };
    bootstrap_relational_counter_table(&repo).await;

    // A real engine-level constraint the read-model schema is unaware of.
    sqlx::query(
        r#"ALTER TABLE "postgres_relational_counter_views"
           ADD CONSTRAINT reject_negative_value CHECK ("value" >= 0)"#,
    )
    .execute(repo.pool())
    .await
    .unwrap();

    let good_id = unique_id("midplan-good");
    let bad_id = unique_id("midplan-bad");
    let mut read_models = ReadModelWritePlanBuilder::new();
    read_models
        .upsert(&RelationalCounterView {
            id: good_id.clone(),
            value: 1,
            counts: HashMap::new(),
        })
        .unwrap();
    read_models
        .upsert(&RelationalCounterView {
            id: bad_id.clone(),
            value: -1,
            counts: HashMap::new(),
        })
        .unwrap();

    let aggregate_id = unique_id("midplan-aggregate");
    let mut projection = CounterProjection::default();
    projection.touch(aggregate_id.clone()).unwrap();
    let identity = StreamIdentity::new(CounterProjection::aggregate_type(), &aggregate_id).unwrap();

    let outbox_id = unique_id("midplan-outbox");
    let outbox_message =
        OutboxMessage::create(&outbox_id, "counter.touched", b"{}".to_vec()).unwrap();

    let err = repo
        .commit_batch(CommitBatch {
            inbox_receipts: Vec::new(),
            streams: vec![StreamWrite::new(identity.clone(), projection.entity_mut())],
            outbox_messages: vec![outbox_message],
            read_model_plans: vec![read_models.into_write_plan().unwrap()],
            snapshots: Vec::new(),
        })
        .await
        .expect_err("a mid-plan constraint violation must fail the commit");
    assert!(
        matches!(err, RepositoryError::Model(_)),
        "expected a Model error from the constraint violation, got {err:?}"
    );

    // 1. Aggregate event absent.
    assert!(
        repo.get_stream(&identity).await.unwrap().is_none(),
        "the aggregate stream must roll back"
    );

    // 2. Outbox row absent.
    let outbox = repo.outbox_store();
    for status in [
        OutboxMessageStatus::Pending,
        OutboxMessageStatus::InFlight,
        OutboxMessageStatus::Published,
        OutboxMessageStatus::Failed,
    ] {
        let rows = outbox.messages_by_status_async(status).await.unwrap();
        assert!(
            rows.iter().all(|m| m.id() != outbox_id),
            "the outbox row must roll back"
        );
    }

    // 3. First read-model mutation absent.
    let good_rows: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*) FROM "postgres_relational_counter_views" WHERE "id" = $1"#,
    )
    .bind(&good_id)
    .fetch_one(repo.pool())
    .await
    .unwrap();
    assert_eq!(good_rows, 0, "the first read-model mutation must roll back");
}

#[tokio::test]
async fn commit_batch_lowers_relational_read_model_plan_into_registered_table() {
    let Some((_schema, repo)) = repository().await else {
        return;
    };
    bootstrap_relational_counter_table(&repo).await;
    let id = unique_id("relational-batch");
    let mut counts = HashMap::new();
    counts.insert("wins".to_string(), 2);
    let view = RelationalCounterView {
        id: id.clone(),
        value: 7,
        counts,
    };
    let mut session = ReadModelWritePlanBuilder::new();
    session.upsert(&view).unwrap();
    let mut projection = CounterProjection::default();
    projection.touch(id.clone()).unwrap();
    let identity = StreamIdentity::new(CounterProjection::aggregate_type(), &id).unwrap();

    repo.read_models(session)
        .commit(&mut projection)
        .await
        .unwrap();

    assert!(repo.get_stream(&identity).await.unwrap().is_some());
    let row = sqlx::query(
        r#"
        SELECT "id", "value", "counts"::text AS counts, "_sourced_version"
        FROM "postgres_relational_counter_views"
        WHERE "id" = $1
        "#,
    )
    .bind(&id)
    .fetch_one(repo.pool())
    .await
    .unwrap();
    let stored_counts: String = sqlx::Row::try_get(&row, "counts").unwrap();
    let stored_counts: serde_json::Value = serde_json::from_str(&stored_counts).unwrap();

    assert_eq!(sqlx::Row::try_get::<String, _>(&row, "id").unwrap(), id);
    assert_eq!(sqlx::Row::try_get::<i64, _>(&row, "value").unwrap(), 7);
    assert_eq!(stored_counts["wins"].as_i64(), Some(2));
    assert_eq!(
        sqlx::Row::try_get::<i64, _>(&row, "_sourced_version").unwrap(),
        1
    );
}

#[tokio::test]
async fn read_model_session_patches_and_deletes_relational_rows() {
    let Some((_schema, repo)) = repository().await else {
        return;
    };
    bootstrap_relational_counter_table(&repo).await;
    let id = unique_id("relational-session");
    let mut counts = HashMap::new();
    counts.insert("wins".to_string(), 2);
    let view = RelationalCounterView {
        id: id.clone(),
        value: 7,
        counts,
    };
    let mut setup = ReadModelWritePlanBuilder::new();
    setup.upsert(&view).unwrap();
    setup.commit(&repo).await.unwrap();

    let mut patched_counts = HashMap::new();
    patched_counts.insert("wins".to_string(), 3);
    patched_counts.insert("losses".to_string(), 1);
    let patch = RowPatch::new()
        .set("value", RowValue::I64(11))
        .set_serde("counts", &patched_counts)
        .unwrap();
    let mut patch_session = ReadModelWritePlanBuilder::new();
    patch_session
        .patch::<RelationalCounterView>(relational_counter_key(&id), patch)
        .unwrap();
    patch_session.commit(&repo).await.unwrap();

    let row = sqlx::query(
        r#"
        SELECT "value", "counts"::text AS counts, "_sourced_version"
        FROM "postgres_relational_counter_views"
        WHERE "id" = $1
        "#,
    )
    .bind(&id)
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
        .delete::<RelationalCounterView>(relational_counter_key(&id))
        .unwrap();
    delete_session.commit(&repo).await.unwrap();

    let remaining: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM "postgres_relational_counter_views"
        WHERE "id" = $1
        "#,
    )
    .bind(&id)
    .fetch_one(repo.pool())
    .await
    .unwrap();
    assert_eq!(remaining, 0);
}

#[tokio::test]
async fn read_model_session_persists_relational_rows() {
    let Some((_schema, repo)) = repository().await else {
        return;
    };
    bootstrap_relational_counter_table(&repo).await;
    let id = unique_id("view");
    let view = RelationalCounterView {
        id: id.clone(),
        value: 42,
        counts: HashMap::new(),
    };
    let mut session = ReadModelWritePlanBuilder::new();
    session.upsert(&view).unwrap();

    let outcome = session.commit(&repo).await.unwrap();
    let row = sqlx::query(
        r#"
        SELECT "value", "_sourced_version"
        FROM "postgres_relational_counter_views"
        WHERE "id" = $1
        "#,
    )
    .bind(&id)
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
    let Some((_schema, repo)) = repository().await else {
        return;
    };
    let id = unique_id("snapshot");
    let counter = StreamIdentity::new("postgres.counter", &id).unwrap();
    let projection = StreamIdentity::new("postgres.counter_projection", &id).unwrap();

    repo.save_snapshot(
        &counter,
        SnapshotRecord::new(
            "postgres.counter",
            id.clone(),
            1,
            "CounterSnapshot",
            1,
            vec![1],
        ),
    )
    .await
    .unwrap();
    repo.save_snapshot(
        &projection,
        SnapshotRecord::new(
            "postgres.counter_projection",
            id,
            2,
            "ProjectionSnapshot",
            1,
            vec![2],
        ),
    )
    .await
    .unwrap();

    let loaded_counter = repo.get_snapshot(&counter).await.unwrap().unwrap();
    let loaded_projection = repo.get_snapshot(&projection).await.unwrap().unwrap();

    assert_eq!(loaded_counter.version, 1);
    assert_eq!(loaded_counter.aggregate_type, "postgres.counter");
    assert_eq!(loaded_counter.snapshot_type, "CounterSnapshot");
    assert_eq!(loaded_counter.payload, vec![1]);
    assert_eq!(loaded_projection.version, 2);
    assert_eq!(
        loaded_projection.aggregate_type,
        "postgres.counter_projection"
    );
    assert_eq!(loaded_projection.snapshot_type, "ProjectionSnapshot");
    assert_eq!(loaded_projection.payload, vec![2]);
}

#[tokio::test]
async fn unsupported_codec_rows_fail_on_read() {
    let Some((_schema, repo)) = repository().await else {
        return;
    };
    let id = unique_id("bad-codec");
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
        VALUES ($1, $2, 1, 'BadEvent', 1, $3, 'json', 1, '{}'::jsonb, now())
        "#,
    )
    .bind("postgres.counter")
    .bind(&id)
    .bind(vec![0_u8])
    .execute(repo.pool())
    .await
    .unwrap();

    let identity = StreamIdentity::new("postgres.counter", &id).unwrap();
    let err = repo.get_stream(&identity).await.unwrap_err();

    assert!(
        matches!(err, RepositoryError::Model(message) if message.contains("unsupported payload codec"))
    );
}

#[tokio::test]
async fn outbox_metadata_columns_round_trip_into_message_metadata() {
    let Some((_schema, repo)) = repository().await else {
        return;
    };
    let message_id = unique_id("outbox-column-metadata");
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
        VALUES ($1, 'OutboxColumns', decode('00', 'hex'), 'bytes', 1, '{}'::jsonb,
                'pending', now(), now(), 0, 'corr-column', 'cause-column')
        "#,
    )
    .bind(&message_id)
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
