#![cfg(feature = "postgres")]

use std::env;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sourced_rust::{
    impl_aggregate, Aggregate, AsyncAggregateBuilder, AsyncCommitBatch, AsyncGetStream,
    AsyncOutboxStore, AsyncSnapshotStore, AsyncStreamWrite, AsyncTransactionalCommit, Entity,
    EventRecord, OutboxMessageStatus, PostgresRepository, ReadModel, ReadModelSession,
    RepositoryError, SnapshotRecord, StreamIdentity,
};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

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

impl_aggregate!(Counter, entity, replay, aggregate_type = "postgres.counter");

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
    aggregate_type = "postgres.counter_projection"
);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct CounterView {
    id: String,
    value: i32,
}

impl ReadModel for CounterView {
    const COLLECTION: &'static str = "postgres_counter_views";

    fn id(&self) -> &str {
        &self.id
    }
}

async fn repository() -> Option<PostgresRepository> {
    let Ok(database_url) = env::var("DATABASE_URL") else {
        eprintln!("skipping Postgres integration test: DATABASE_URL is not set");
        return None;
    };

    Some(
        PostgresRepository::connect_and_migrate(&database_url)
            .await
            .unwrap(),
    )
}

fn unique_id(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{nanos}-{id}")
}

#[tokio::test]
async fn migration_is_idempotent_and_uses_postgres_column_types() {
    let Some(repo) = repository().await else {
        return;
    };
    repo.migrate().await.unwrap();

    let rows = sqlx::query(
        r#"
        SELECT column_name, udt_name
        FROM information_schema.columns
        WHERE table_name = 'aggregate_events'
          AND column_name IN ('payload', 'metadata', 'recorded_at')
        "#,
    )
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

    let read_model_table: Option<String> =
        sqlx::query_scalar("SELECT to_regclass('public.transactional_read_models')::text")
            .fetch_one(repo.pool())
            .await
            .unwrap();
    assert!(read_model_table.is_none());
}

#[tokio::test]
async fn aggregate_stream_round_trips_with_metadata() {
    let Some(repo) = repository().await else {
        return;
    };
    let counter_repo = repo.clone().async_aggregate::<Counter>();
    let id = unique_id("counter");

    let mut counter = Counter::default();
    counter.entity.set_correlation_id("corr-postgres");
    counter.increment(&id, 2);
    counter.increment(&id, 3);

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
    let Some(repo) = repository().await else {
        return;
    };
    let counter_repo = repo.clone().async_aggregate::<Counter>();
    let counter_id = unique_id("conflict");
    let other_id = unique_id("rollback");

    let mut original = Counter::default();
    original.increment(&counter_id, 1);
    counter_repo.commit(&mut original).await.unwrap();

    let mut stale = counter_repo.get(&counter_id).await.unwrap().unwrap();
    let mut winner = counter_repo.get(&counter_id).await.unwrap().unwrap();
    stale.increment(&counter_id, 10);
    winner.increment(&counter_id, 20);
    counter_repo.commit(&mut winner).await.unwrap();

    let mut other = CounterProjection::default();
    other.touch(&other_id);

    let stale_identity = StreamIdentity::new(Counter::aggregate_type(), &counter_id).unwrap();
    let other_identity =
        StreamIdentity::new(CounterProjection::aggregate_type(), &other_id).unwrap();
    let err = repo
        .commit_batch_async(AsyncCommitBatch {
            streams: vec![
                AsyncStreamWrite::new(stale_identity, stale.entity_mut()),
                AsyncStreamWrite::new(other_identity.clone(), other.entity_mut()),
            ],
            outbox_messages: Vec::new(),
            read_model_plans: Vec::new(),
            snapshots: vec![sourced_rust::AsyncSnapshotWrite::Save {
                identity: other_identity.clone(),
                record: SnapshotRecord {
                    aggregate_id: other_id.clone(),
                    version: 1,
                    data: vec![1],
                },
            }],
        })
        .await
        .unwrap_err();

    assert!(matches!(err, RepositoryError::ConcurrentWrite { .. }));
    assert!(repo.get_stream(&other_identity).await.unwrap().is_none());
    assert!(repo
        .get_snapshot_async(&other_identity)
        .await
        .unwrap()
        .is_none());
    assert_eq!(stale.entity().committed_version(), 1);
    assert_eq!(stale.entity().new_events().len(), 1);
}

#[tokio::test]
async fn duplicate_stream_identity_is_rejected_before_sql_writes() {
    let Some(repo) = repository().await else {
        return;
    };
    let id = unique_id("duplicate");
    let identity = StreamIdentity::new(Counter::aggregate_type(), &id).unwrap();
    let mut first = Entity::with_id(&id);
    first.digest_empty("First").unwrap();
    let mut second = Entity::with_id(&id);
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
            id: format!("{}:{id}", Counter::aggregate_type())
        }
    );
    assert!(repo.get_stream(&identity).await.unwrap().is_none());
}

#[tokio::test]
async fn read_model_plans_are_rejected_in_first_pass() {
    let Some(repo) = repository().await else {
        return;
    };
    let id = unique_id("read-model");
    let mut entity = Entity::with_id(&id);
    entity.digest_empty("Touched").unwrap();
    let identity = StreamIdentity::new(Counter::aggregate_type(), &id).unwrap();
    let mut session = ReadModelSession::new();
    session.document(&CounterView { id, value: 1 }).unwrap();

    let err = repo
        .commit_batch_async(AsyncCommitBatch {
            streams: vec![AsyncStreamWrite::new(identity.clone(), &mut entity)],
            outbox_messages: Vec::new(),
            read_model_plans: vec![session.into_write_plan().unwrap()],
            snapshots: Vec::new(),
        })
        .await
        .unwrap_err();

    assert!(
        matches!(err, RepositoryError::Model(message) if message.contains("does not persist read-model write plans"))
    );
    assert!(repo.get_stream(&identity).await.unwrap().is_none());
}

#[tokio::test]
async fn snapshots_persist_by_full_stream_identity() {
    let Some(repo) = repository().await else {
        return;
    };
    let id = unique_id("snapshot");
    let counter = StreamIdentity::new("postgres.counter", &id).unwrap();
    let projection = StreamIdentity::new("postgres.counter_projection", &id).unwrap();

    repo.save_snapshot_async(
        &counter,
        SnapshotRecord {
            aggregate_id: id.clone(),
            version: 1,
            data: vec![1],
        },
    )
    .await
    .unwrap();
    repo.save_snapshot_async(
        &projection,
        SnapshotRecord {
            aggregate_id: id,
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
    let Some(repo) = repository().await else {
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
    let Some(repo) = repository().await else {
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
