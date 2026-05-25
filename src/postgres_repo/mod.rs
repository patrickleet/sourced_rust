//! Postgres-backed async aggregate repository.
//!
//! This adapter is the production-oriented SQL event-store path. It is
//! feature-gated behind `postgres`, async-only, and intentionally does not
//! create read-model tables in the first pass.

#![expect(
    clippy::manual_async_fn,
    reason = "async trait impls return impl Future + Send to preserve public Send bounds"
)]

use std::collections::HashSet;
use std::future::Future;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sqlx::postgres::{PgPoolOptions, PgRow};
use sqlx::{PgPool, Postgres, Row, Transaction};

use crate::entity::{
    Entity, EventRecord, EventRecordError, BITCODE_PAYLOAD_CODEC, BITCODE_PAYLOAD_CODEC_VERSION,
};
use crate::repository::{
    AsyncCommitBatch, AsyncGetStream, AsyncSnapshotStore, AsyncSnapshotWrite, AsyncStreamWrite,
    AsyncTransactionalCommit, PreparedEventAppend, RepositoryError, StreamIdentity,
};
use crate::snapshot::SnapshotRecord;

const POSTGRES_SCHEMA: &str = include_str!("../../migrations/postgres/0001_initial.sql");

/// Postgres-backed async repository.
#[derive(Clone)]
pub struct PostgresRepository {
    pool: PgPool,
}

impl PostgresRepository {
    /// Create a repository from an existing migrated pool.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Open a Postgres pool without applying migrations.
    pub async fn connect(database_url: &str) -> Result<Self, RepositoryError> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await
            .map_err(|err| repository_storage_error("connect", err))?;
        Ok(Self::new(pool))
    }

    /// Open a Postgres pool and apply the explicit Postgres migrations.
    pub async fn connect_and_migrate(database_url: &str) -> Result<Self, RepositoryError> {
        let repo = Self::connect(database_url).await?;
        repo.migrate().await?;
        Ok(repo)
    }

    /// Apply Postgres migrations to this repository's pool.
    pub async fn migrate(&self) -> Result<(), RepositoryError> {
        Self::migrate_pool(&self.pool).await
    }

    /// Apply Postgres migrations to an existing pool.
    pub async fn migrate_pool(pool: &PgPool) -> Result<(), RepositoryError> {
        for statement in POSTGRES_SCHEMA.split(';') {
            let statement = statement.trim();
            if statement.is_empty() {
                continue;
            }
            sqlx::query(statement)
                .execute(pool)
                .await
                .map_err(|err| repository_storage_error("migrate", err))?;
        }
        Ok(())
    }

    /// Access the underlying SQLx pool for application-specific setup or tests.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

impl AsyncGetStream for PostgresRepository {
    fn get_stream<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<Option<Entity>, RepositoryError>> + Send + 'a {
        async move {
            let rows = sqlx::query(
                r#"
                SELECT event_name,
                       event_version,
                       payload,
                       payload_codec,
                       payload_codec_version,
                       metadata::text AS metadata,
                       sequence,
                       EXTRACT(EPOCH FROM recorded_at)::double precision AS recorded_at_epoch
                FROM aggregate_events
                WHERE aggregate_type = $1 AND aggregate_id = $2
                ORDER BY sequence ASC
                "#,
            )
            .bind(identity.aggregate_type())
            .bind(identity.aggregate_id())
            .fetch_all(&self.pool)
            .await
            .map_err(|err| repository_storage_error("load stream", err))?;

            if rows.is_empty() {
                return Ok(None);
            }

            let mut events = Vec::with_capacity(rows.len());
            for row in rows {
                events.push(event_from_row(row)?);
            }

            let mut entity = Entity::new();
            entity.set_id(identity.aggregate_id());
            entity.load_from_history(events);
            Ok(Some(entity))
        }
    }

    fn get_streams<'a>(
        &'a self,
        identities: &'a [StreamIdentity],
    ) -> impl Future<Output = Result<Vec<Entity>, RepositoryError>> + Send + 'a {
        async move {
            let mut entities = Vec::with_capacity(identities.len());
            for identity in identities {
                if let Some(entity) = self.get_stream(identity).await? {
                    entities.push(entity);
                }
            }
            Ok(entities)
        }
    }
}

impl AsyncTransactionalCommit for PostgresRepository {
    fn commit_batch_async<'a>(
        &'a self,
        batch: AsyncCommitBatch<'a>,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        async move {
            reject_duplicate_streams(&batch.streams)?;
            validate_entity_id_matches_identity(&batch.streams)?;
            reject_read_model_plans(&batch)?;

            let prepared = batch
                .streams
                .iter()
                .map(PreparedEventAppend::from_stream_write)
                .collect::<Vec<_>>();
            validate_prepared_appends(&prepared)?;

            let mut tx = self
                .pool
                .begin()
                .await
                .map_err(|err| repository_storage_error("begin commit transaction", err))?;

            for append in &prepared {
                let actual = stream_version_in_tx(&mut tx, &append.identity).await?;
                if actual != append.expected_version {
                    return Err(RepositoryError::ConcurrentWrite {
                        id: append.identity.to_string(),
                        expected: append.expected_version,
                        actual,
                    });
                }
            }

            for append in &prepared {
                for event in &append.events {
                    insert_event_in_tx(
                        &self.pool,
                        &mut tx,
                        &append.identity,
                        append.expected_version,
                        event,
                    )
                    .await?;
                }
            }

            for write in batch.snapshots {
                match write {
                    AsyncSnapshotWrite::Save { identity, record } => {
                        save_snapshot_in_tx(&mut tx, &identity, record).await?;
                    }
                }
            }

            tx.commit()
                .await
                .map_err(|err| repository_storage_error("commit transaction", err))?;

            for stream in batch.streams {
                stream.entity.mark_committed();
            }

            Ok(())
        }
    }
}

impl AsyncSnapshotStore for PostgresRepository {
    fn get_snapshot_async<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<Option<SnapshotRecord>, RepositoryError>> + Send + 'a {
        async move {
            let row = sqlx::query(
                r#"
                SELECT aggregate_id, version, data
                FROM aggregate_snapshots
                WHERE aggregate_type = $1 AND aggregate_id = $2
                "#,
            )
            .bind(identity.aggregate_type())
            .bind(identity.aggregate_id())
            .fetch_optional(&self.pool)
            .await
            .map_err(|err| repository_storage_error("load snapshot", err))?;

            let Some(row) = row else {
                return Ok(None);
            };

            Ok(Some(snapshot_from_row(row)?))
        }
    }

    fn save_snapshot_async<'a>(
        &'a self,
        identity: &'a StreamIdentity,
        record: SnapshotRecord,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        async move {
            let mut tx = self
                .pool
                .begin()
                .await
                .map_err(|err| repository_storage_error("begin snapshot transaction", err))?;
            save_snapshot_in_tx(&mut tx, identity, record).await?;
            tx.commit()
                .await
                .map_err(|err| repository_storage_error("commit snapshot transaction", err))?;
            Ok(())
        }
    }

    fn delete_snapshot_async<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<bool, RepositoryError>> + Send + 'a {
        async move {
            let result = sqlx::query(
                r#"
                DELETE FROM aggregate_snapshots
                WHERE aggregate_type = $1 AND aggregate_id = $2
                "#,
            )
            .bind(identity.aggregate_type())
            .bind(identity.aggregate_id())
            .execute(&self.pool)
            .await
            .map_err(|err| repository_storage_error("delete snapshot", err))?;

            Ok(result.rows_affected() > 0)
        }
    }
}

fn reject_duplicate_streams(streams: &[AsyncStreamWrite<'_>]) -> Result<(), RepositoryError> {
    let mut seen = HashSet::with_capacity(streams.len());
    for stream in streams {
        let key = stream.identity.storage_key();
        if !seen.insert(key) {
            return Err(RepositoryError::DuplicateStreamInBatch {
                id: stream.identity.to_string(),
            });
        }
    }
    Ok(())
}

fn validate_entity_id_matches_identity(
    streams: &[AsyncStreamWrite<'_>],
) -> Result<(), RepositoryError> {
    for stream in streams {
        if stream.entity.id() != stream.identity.aggregate_id() {
            return Err(RepositoryError::Model(format!(
                "stream identity `{}` does not match entity id `{}`",
                stream.identity,
                stream.entity.id()
            )));
        }
    }
    Ok(())
}

fn reject_read_model_plans(batch: &AsyncCommitBatch<'_>) -> Result<(), RepositoryError> {
    if batch.read_model_plans.iter().any(|plan| !plan.is_empty()) {
        return Err(RepositoryError::Model(
            "PostgresRepository first pass does not persist read-model write plans".into(),
        ));
    }
    Ok(())
}

fn validate_prepared_appends(appends: &[PreparedEventAppend]) -> Result<(), RepositoryError> {
    for append in appends {
        for (offset, event) in append.events.iter().enumerate() {
            validate_supported_event_codec(event)?;
            let expected_sequence = append.expected_version + offset as u64 + 1;
            if event.sequence != expected_sequence {
                return Err(RepositoryError::Model(format!(
                    "event `{}` for stream `{}` has sequence {}, expected {}",
                    event.event_name, append.identity, event.sequence, expected_sequence
                )));
            }
        }
    }
    Ok(())
}

fn validate_supported_event_codec(event: &EventRecord) -> Result<(), RepositoryError> {
    if event.payload_codec != BITCODE_PAYLOAD_CODEC
        || event.payload_codec_version != BITCODE_PAYLOAD_CODEC_VERSION
    {
        return Err(EventRecordError::unsupported_codec(
            &event.payload_codec,
            event.payload_codec_version,
        )
        .into());
    }
    Ok(())
}

async fn stream_version_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    identity: &StreamIdentity,
) -> Result<u64, RepositoryError> {
    let row = sqlx::query(
        r#"
        SELECT MAX(sequence) AS version
        FROM aggregate_events
        WHERE aggregate_type = $1 AND aggregate_id = $2
        "#,
    )
    .bind(identity.aggregate_type())
    .bind(identity.aggregate_id())
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| repository_storage_error("load stream version", err))?;

    let version: Option<i64> = row
        .try_get("version")
        .map_err(|err| repository_storage_error("decode stream version row", err))?;
    version
        .map(|value| repository_u64_from_i64(value, "sequence"))
        .unwrap_or(Ok(0))
}

async fn stream_version_pool(
    pool: &PgPool,
    identity: &StreamIdentity,
) -> Result<u64, RepositoryError> {
    let row = sqlx::query(
        r#"
        SELECT MAX(sequence) AS version
        FROM aggregate_events
        WHERE aggregate_type = $1 AND aggregate_id = $2
        "#,
    )
    .bind(identity.aggregate_type())
    .bind(identity.aggregate_id())
    .fetch_one(pool)
    .await
    .map_err(|err| repository_storage_error("load stream version", err))?;

    let version: Option<i64> = row
        .try_get("version")
        .map_err(|err| repository_storage_error("decode stream version row", err))?;
    version
        .map(|value| repository_u64_from_i64(value, "sequence"))
        .unwrap_or(Ok(0))
}

async fn insert_event_in_tx(
    pool: &PgPool,
    tx: &mut Transaction<'_, Postgres>,
    identity: &StreamIdentity,
    expected_version: u64,
    event: &EventRecord,
) -> Result<(), RepositoryError> {
    let metadata = serde_json::to_string(&event.metadata)
        .map_err(|err| RepositoryError::Model(format!("serialize event metadata: {err}")))?;

    let result = sqlx::query(
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
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9::jsonb, to_timestamp($10))
        "#,
    )
    .bind(identity.aggregate_type())
    .bind(identity.aggregate_id())
    .bind(repository_i64_from_u64(event.sequence, "sequence")?)
    .bind(&event.event_name)
    .bind(repository_i32_from_u64(
        event.event_version,
        "event_version",
    )?)
    .bind(&event.payload)
    .bind(&event.payload_codec)
    .bind(i32::from(event.payload_codec_version))
    .bind(metadata)
    .bind(system_time_to_epoch_secs(event.timestamp)?)
    .execute(&mut **tx)
    .await;

    match result {
        Ok(_) => Ok(()),
        Err(err) if is_unique_violation(&err) => {
            let actual = stream_version_pool(pool, identity)
                .await
                .unwrap_or_default();
            Err(RepositoryError::ConcurrentWrite {
                id: identity.to_string(),
                expected: expected_version,
                actual,
            })
        }
        Err(err) => Err(repository_storage_error("insert event", err)),
    }
}

fn event_from_row(row: PgRow) -> Result<EventRecord, RepositoryError> {
    let payload_codec: String = row
        .try_get("payload_codec")
        .map_err(|err| repository_storage_error("decode payload codec row", err))?;
    let payload_codec_version = repository_u16_from_i32(
        row.try_get("payload_codec_version")
            .map_err(|err| repository_storage_error("decode payload codec version row", err))?,
        "payload_codec_version",
    )?;
    let metadata_json: String = row
        .try_get("metadata")
        .map_err(|err| repository_storage_error("decode metadata row", err))?;
    let metadata = serde_json::from_str(&metadata_json)
        .map_err(|err| RepositoryError::Model(format!("deserialize event metadata: {err}")))?;
    let event = EventRecord {
        event_name: row
            .try_get("event_name")
            .map_err(|err| repository_storage_error("decode event name row", err))?,
        payload_codec,
        payload_codec_version,
        payload: row
            .try_get("payload")
            .map_err(|err| repository_storage_error("decode payload row", err))?,
        event_version: repository_u64_from_i32(
            row.try_get("event_version")
                .map_err(|err| repository_storage_error("decode event version row", err))?,
            "event_version",
        )?,
        sequence: repository_u64_from_i64(
            row.try_get("sequence")
                .map_err(|err| repository_storage_error("decode sequence row", err))?,
            "sequence",
        )?,
        timestamp: system_time_from_epoch_secs(
            row.try_get("recorded_at_epoch")
                .map_err(|err| repository_storage_error("decode recorded_at row", err))?,
        )?,
        metadata,
    };
    validate_supported_event_codec(&event)?;
    Ok(event)
}

async fn save_snapshot_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    identity: &StreamIdentity,
    record: SnapshotRecord,
) -> Result<(), RepositoryError> {
    if record.aggregate_id != identity.aggregate_id() {
        return Err(RepositoryError::Model(format!(
            "snapshot aggregate id `{}` does not match stream identity `{}`",
            record.aggregate_id, identity
        )));
    }

    sqlx::query(
        r#"
        INSERT INTO aggregate_snapshots (aggregate_type, aggregate_id, version, data)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT(aggregate_type, aggregate_id) DO UPDATE SET
          version = excluded.version,
          data = excluded.data,
          updated_at = now()
        "#,
    )
    .bind(identity.aggregate_type())
    .bind(identity.aggregate_id())
    .bind(repository_i64_from_u64(record.version, "snapshot version")?)
    .bind(record.data)
    .execute(&mut **tx)
    .await
    .map_err(|err| repository_storage_error("save snapshot", err))?;

    Ok(())
}

fn snapshot_from_row(row: PgRow) -> Result<SnapshotRecord, RepositoryError> {
    Ok(SnapshotRecord {
        aggregate_id: row
            .try_get("aggregate_id")
            .map_err(|err| repository_storage_error("decode snapshot aggregate id row", err))?,
        version: repository_u64_from_i64(
            row.try_get("version")
                .map_err(|err| repository_storage_error("decode snapshot version row", err))?,
            "snapshot version",
        )?,
        data: row
            .try_get("data")
            .map_err(|err| repository_storage_error("decode snapshot data row", err))?,
    })
}

fn repository_i64_from_u64(value: u64, field: &str) -> Result<i64, RepositoryError> {
    i64::try_from(value).map_err(|_| {
        RepositoryError::Model(format!(
            "postgres {field} value {value} exceeds bigint storage"
        ))
    })
}

fn repository_i32_from_u64(value: u64, field: &str) -> Result<i32, RepositoryError> {
    i32::try_from(value).map_err(|_| {
        RepositoryError::Model(format!(
            "postgres {field} value {value} exceeds integer storage"
        ))
    })
}

fn repository_u64_from_i64(value: i64, field: &str) -> Result<u64, RepositoryError> {
    u64::try_from(value)
        .map_err(|_| RepositoryError::Model(format!("postgres {field} value {value} is negative")))
}

fn repository_u64_from_i32(value: i32, field: &str) -> Result<u64, RepositoryError> {
    u64::try_from(value)
        .map_err(|_| RepositoryError::Model(format!("postgres {field} value {value} is negative")))
}

fn repository_u16_from_i32(value: i32, field: &str) -> Result<u16, RepositoryError> {
    u16::try_from(value)
        .map_err(|_| RepositoryError::Model(format!("postgres {field} value {value} is invalid")))
}

fn system_time_to_epoch_secs(timestamp: SystemTime) -> Result<f64, RepositoryError> {
    let duration = timestamp.duration_since(UNIX_EPOCH).map_err(|err| {
        RepositoryError::Model(format!(
            "event timestamp before UNIX epoch cannot be stored in postgres: {err}"
        ))
    })?;
    Ok(duration.as_secs_f64())
}

fn system_time_from_epoch_secs(value: f64) -> Result<SystemTime, RepositoryError> {
    if !value.is_finite() || value < 0.0 {
        return Err(RepositoryError::Model(format!(
            "postgres recorded_at epoch value {value} is invalid"
        )));
    }
    Ok(UNIX_EPOCH + Duration::from_secs_f64(value))
}

fn is_unique_violation(err: &sqlx::Error) -> bool {
    match err {
        sqlx::Error::Database(db_err) => db_err.code().as_deref() == Some("23505"),
        _ => false,
    }
}

fn repository_storage_error(operation: &str, err: sqlx::Error) -> RepositoryError {
    RepositoryError::Model(format!("postgres {operation} failed: {err}"))
}
