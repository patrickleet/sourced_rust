//! Postgres-backed async aggregate repository.
//!
//! This adapter is the production-oriented SQL event-store path. It is
//! feature-gated behind `postgres`, async-only, and intentionally does not
//! create read-model tables in the first pass.

#![expect(
    clippy::manual_async_fn,
    reason = "async trait impls return impl Future + Send to preserve public Send bounds"
)]

use std::future::Future;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sqlx::postgres::{PgPoolOptions, PgRow};
use sqlx::{PgPool, Postgres, Row, Transaction};

use crate::entity::Entity;
use crate::entity::EventRecord;
use crate::outbox::{OutboxMessage, OutboxMessageStatus};
use crate::outbox_worker::{ensure_active_claim, OutboxPublishFailureAction};
use crate::repository::{
    AsyncCommitBatch, AsyncGetStream, AsyncOutboxRepositoryExt, AsyncSnapshotStore,
    AsyncSnapshotWrite, AsyncTransactionalCommit, PreparedEventAppend, RepositoryError,
    StreamIdentity,
};
use crate::snapshot::SnapshotRecord;
use crate::sqlx_repo::{
    self, deserialize_event_metadata, is_postgres_unique_violation,
    reject_duplicate_outbox_messages, reject_duplicate_streams,
    repository_i32_from_u64 as sqlx_repository_i32_from_u64,
    repository_i64_from_u64 as sqlx_repository_i64_from_u64,
    repository_u16_from_i32 as sqlx_repository_u16_from_i32,
    repository_u64_from_i32 as sqlx_repository_u64_from_i32,
    repository_u64_from_i64 as sqlx_repository_u64_from_i64, serialize_event_metadata,
    validate_entity_id_matches_identity, validate_prepared_appends, validate_snapshot_identity,
    validate_supported_event_codec,
};

const POSTGRES_SCHEMA: &str = include_str!("../../migrations/postgres/0001_initial.sql");
const POSTGRES_BACKEND: &str = "postgres";
const BIGINT_STORAGE: &str = "bigint storage";
const INTEGER_STORAGE: &str = "integer storage";

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
            reject_duplicate_outbox_messages(&batch.outbox_messages)?;
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

            for message in &batch.outbox_messages {
                insert_outbox_message_in_tx(&mut tx, message).await?;
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

impl AsyncOutboxRepositoryExt for PostgresRepository {
    fn outbox_messages_by_status_async(
        &self,
        status: OutboxMessageStatus,
    ) -> impl Future<Output = Result<Vec<OutboxMessage>, RepositoryError>> + Send + '_ {
        async move {
            let rows = sqlx::query(outbox_message_select_by_status_sql())
                .bind(status.as_str())
                .fetch_all(&self.pool)
                .await
                .map_err(|err| repository_storage_error("load outbox messages by status", err))?;

            rows.into_iter().map(outbox_message_from_row).collect()
        }
    }

    fn claim_outbox_messages_async<'a>(
        &'a self,
        worker_id: &'a str,
        max: usize,
        lease: Duration,
    ) -> impl Future<Output = Result<Vec<OutboxMessage>, RepositoryError>> + Send + 'a {
        async move {
            if max == 0 {
                return Ok(Vec::new());
            }

            let now = SystemTime::now();
            let now_epoch = system_time_to_epoch_secs(now)?;
            let claimed_until = now.checked_add(lease).ok_or_else(|| {
                RepositoryError::Model("failed to compute outbox lease deadline".into())
            })?;
            let claimed_until_epoch = system_time_to_epoch_secs(claimed_until)?;
            let limit = sqlx_repository_i64_from_u64(
                POSTGRES_BACKEND,
                max as u64,
                "outbox claim limit",
                BIGINT_STORAGE,
            )?;

            let mut tx =
                self.pool.begin().await.map_err(|err| {
                    repository_storage_error("begin outbox claim transaction", err)
                })?;

            let rows = sqlx::query(
                r#"
                WITH candidates AS (
                  SELECT message_id
                  FROM outbox_messages
                  WHERE (status = $1 AND next_available_at <= to_timestamp($2))
                     OR (status = $3 AND (claimed_until IS NULL OR claimed_until <= to_timestamp($2)))
                  ORDER BY created_at ASC, message_id ASC
                  LIMIT $4
                  FOR UPDATE SKIP LOCKED
                )
                UPDATE outbox_messages AS message
                SET status = $5,
                    claimed_by = $6,
                    claimed_until = to_timestamp($7),
                    attempts = attempts + 1,
                    updated_at = now()
                FROM candidates
                WHERE message.message_id = candidates.message_id
                RETURNING message.message_id,
                          message.event_type,
                          message.payload,
                          message.payload_codec,
                          message.payload_codec_version,
                          message.metadata::text AS metadata,
                          message.status,
                          EXTRACT(EPOCH FROM message.created_at)::double precision AS created_at_epoch,
                          message.claimed_by,
                          EXTRACT(EPOCH FROM message.claimed_until)::double precision AS claimed_until_epoch,
                          message.attempts,
                          message.last_error,
                          message.destination,
                          message.source_aggregate_type,
                          message.source_aggregate_id,
                          message.source_sequence,
                          message.correlation_id,
                          message.causation_id
                "#,
            )
            .bind(OutboxMessageStatus::Pending.as_str())
            .bind(now_epoch)
            .bind(OutboxMessageStatus::InFlight.as_str())
            .bind(limit)
            .bind(OutboxMessageStatus::InFlight.as_str())
            .bind(worker_id)
            .bind(claimed_until_epoch)
            .fetch_all(&mut *tx)
            .await
            .map_err(|err| repository_storage_error("claim outbox messages", err))?;

            tx.commit()
                .await
                .map_err(|err| repository_storage_error("commit outbox claim transaction", err))?;

            rows.into_iter().map(outbox_message_from_row).collect()
        }
    }

    fn complete_outbox_message_for_worker_async<'a>(
        &'a self,
        message_id: &'a str,
        worker_id: &'a str,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        async move {
            let now = SystemTime::now();
            let now_epoch = system_time_to_epoch_secs(now)?;
            let result = sqlx::query(
                r#"
                UPDATE outbox_messages
                SET status = $1,
                    claimed_by = NULL,
                    claimed_until = NULL,
                    published_at = to_timestamp($2),
                    updated_at = now()
                WHERE message_id = $3
                  AND status = $4
                  AND claimed_by = $5
                  AND claimed_until IS NOT NULL
                  AND claimed_until > to_timestamp($6)
                "#,
            )
            .bind(OutboxMessageStatus::Published.as_str())
            .bind(now_epoch)
            .bind(message_id)
            .bind(OutboxMessageStatus::InFlight.as_str())
            .bind(worker_id)
            .bind(now_epoch)
            .execute(&self.pool)
            .await
            .map_err(|err| repository_storage_error("complete outbox message", err))?;

            ensure_outbox_update_applied(
                &self.pool,
                result.rows_affected(),
                message_id,
                |message| ensure_active_claim(message, Some(worker_id), now),
            )
            .await
        }
    }

    fn release_outbox_message_for_worker_async<'a>(
        &'a self,
        message_id: &'a str,
        worker_id: &'a str,
        error: &'a str,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        async move {
            let now = SystemTime::now();
            let now_epoch = system_time_to_epoch_secs(now)?;
            let result = sqlx::query(
                r#"
                UPDATE outbox_messages
                SET status = $1,
                    claimed_by = NULL,
                    claimed_until = NULL,
                    next_available_at = to_timestamp($2),
                    last_error = $3,
                    updated_at = now()
                WHERE message_id = $4
                  AND status = $5
                  AND claimed_by = $6
                  AND claimed_until IS NOT NULL
                  AND claimed_until > to_timestamp($7)
                "#,
            )
            .bind(OutboxMessageStatus::Pending.as_str())
            .bind(now_epoch)
            .bind(empty_string_as_none(error))
            .bind(message_id)
            .bind(OutboxMessageStatus::InFlight.as_str())
            .bind(worker_id)
            .bind(now_epoch)
            .execute(&self.pool)
            .await
            .map_err(|err| repository_storage_error("release outbox message", err))?;

            ensure_outbox_update_applied(
                &self.pool,
                result.rows_affected(),
                message_id,
                |message| ensure_active_claim(message, Some(worker_id), now),
            )
            .await
        }
    }

    fn fail_outbox_message_for_worker_async<'a>(
        &'a self,
        message_id: &'a str,
        worker_id: &'a str,
        error: &'a str,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        async move {
            let now = SystemTime::now();
            let now_epoch = system_time_to_epoch_secs(now)?;
            let result = sqlx::query(
                r#"
                UPDATE outbox_messages
                SET status = $1,
                    claimed_by = NULL,
                    claimed_until = NULL,
                    last_error = $2,
                    failed_at = to_timestamp($3),
                    updated_at = now()
                WHERE message_id = $4
                  AND status = $5
                  AND claimed_by = $6
                  AND claimed_until IS NOT NULL
                  AND claimed_until > to_timestamp($7)
                "#,
            )
            .bind(OutboxMessageStatus::Failed.as_str())
            .bind(empty_string_as_none(error))
            .bind(now_epoch)
            .bind(message_id)
            .bind(OutboxMessageStatus::InFlight.as_str())
            .bind(worker_id)
            .bind(now_epoch)
            .execute(&self.pool)
            .await
            .map_err(|err| repository_storage_error("fail outbox message", err))?;

            ensure_outbox_update_applied(
                &self.pool,
                result.rows_affected(),
                message_id,
                |message| ensure_active_claim(message, Some(worker_id), now),
            )
            .await
        }
    }

    fn record_outbox_publish_failure_async<'a>(
        &'a self,
        message_id: &'a str,
        worker_id: &'a str,
        error: &'a str,
        max_attempts: u32,
    ) -> impl Future<Output = Result<OutboxPublishFailureAction, RepositoryError>> + Send + 'a {
        async move {
            let message = outbox_message_by_id_pool(&self.pool, message_id)
                .await?
                .ok_or_else(|| RepositoryError::NotFound {
                    id: message_id.to_string(),
                })?;
            ensure_active_claim(&message, Some(worker_id), SystemTime::now())?;

            if message.attempts >= max_attempts {
                self.fail_outbox_message_for_worker_async(message_id, worker_id, error)
                    .await?;
                Ok(OutboxPublishFailureAction::Failed)
            } else {
                self.release_outbox_message_for_worker_async(message_id, worker_id, error)
                    .await?;
                Ok(OutboxPublishFailureAction::Released)
            }
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

fn reject_read_model_plans(batch: &AsyncCommitBatch<'_>) -> Result<(), RepositoryError> {
    if batch.read_model_plans.iter().any(|plan| !plan.is_empty()) {
        return Err(RepositoryError::Model(
            "PostgresRepository first pass does not persist read-model write plans".into(),
        ));
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
        .map(|value| sqlx_repository_u64_from_i64(POSTGRES_BACKEND, value, "sequence"))
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
        .map(|value| sqlx_repository_u64_from_i64(POSTGRES_BACKEND, value, "sequence"))
        .unwrap_or(Ok(0))
}

async fn insert_event_in_tx(
    pool: &PgPool,
    tx: &mut Transaction<'_, Postgres>,
    identity: &StreamIdentity,
    expected_version: u64,
    event: &EventRecord,
) -> Result<(), RepositoryError> {
    let metadata = serialize_event_metadata(&event.metadata)?;

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
    .bind(sqlx_repository_i64_from_u64(
        POSTGRES_BACKEND,
        event.sequence,
        "sequence",
        BIGINT_STORAGE,
    )?)
    .bind(&event.event_name)
    .bind(sqlx_repository_i32_from_u64(
        POSTGRES_BACKEND,
        event.event_version,
        "event_version",
        INTEGER_STORAGE,
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
        Err(err) if is_postgres_unique_violation(&err) => {
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

async fn insert_outbox_message_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    message: &OutboxMessage,
) -> Result<(), RepositoryError> {
    let metadata = serialize_event_metadata(&message.metadata)?;
    let source_sequence = message
        .source_sequence
        .map(|value| {
            sqlx_repository_i64_from_u64(
                POSTGRES_BACKEND,
                value,
                "outbox source sequence",
                BIGINT_STORAGE,
            )
        })
        .transpose()?;
    let result = sqlx::query(
        r#"
        INSERT INTO outbox_messages (
          message_id,
          event_type,
          payload,
          payload_codec,
          payload_codec_version,
          destination,
          metadata,
          status,
          created_at,
          next_available_at,
          claimed_by,
          claimed_until,
          attempts,
          last_error,
          source_aggregate_type,
          source_aggregate_id,
          source_sequence,
          correlation_id,
          causation_id
        )
        VALUES (
          $1, $2, $3, $4, $5, $6, $7::jsonb, $8,
          to_timestamp($9), to_timestamp($10), $11,
          to_timestamp($12::double precision), $13, $14,
          $15, $16, $17, $18, $19
        )
        "#,
    )
    .bind(message.id())
    .bind(&message.event_type)
    .bind(&message.payload)
    .bind(&message.payload_codec)
    .bind(i32::from(message.payload_codec_version))
    .bind(&message.destination)
    .bind(metadata)
    .bind(message.status.as_str())
    .bind(system_time_to_epoch_secs(message.created_at)?)
    .bind(system_time_to_epoch_secs(message.created_at)?)
    .bind(&message.worker_id)
    .bind(
        message
            .leased_until
            .map(system_time_to_epoch_secs)
            .transpose()?,
    )
    .bind(sqlx_repository_i32_from_u64(
        POSTGRES_BACKEND,
        u64::from(message.attempts),
        "outbox attempts",
        INTEGER_STORAGE,
    )?)
    .bind(&message.last_error)
    .bind(&message.source_aggregate_type)
    .bind(&message.source_aggregate_id)
    .bind(source_sequence)
    .bind(message.correlation_id())
    .bind(message.causation_id())
    .execute(&mut **tx)
    .await;

    match result {
        Ok(_) => Ok(()),
        Err(err) if is_postgres_unique_violation(&err) => {
            Err(RepositoryError::DuplicateOutboxMessageInBatch {
                id: message.id().to_string(),
            })
        }
        Err(err) => Err(repository_storage_error("insert outbox message", err)),
    }
}

async fn outbox_message_by_id_pool(
    pool: &PgPool,
    message_id: &str,
) -> Result<Option<OutboxMessage>, RepositoryError> {
    let row = sqlx::query(outbox_message_select_by_id_sql())
        .bind(message_id)
        .fetch_optional(pool)
        .await
        .map_err(|err| repository_storage_error("load outbox message", err))?;
    row.map(outbox_message_from_row).transpose()
}

fn outbox_message_select_by_status_sql() -> &'static str {
    r#"
    SELECT message_id,
           event_type,
           payload,
           payload_codec,
           payload_codec_version,
           metadata::text AS metadata,
           status,
           EXTRACT(EPOCH FROM created_at)::double precision AS created_at_epoch,
           claimed_by,
           EXTRACT(EPOCH FROM claimed_until)::double precision AS claimed_until_epoch,
           attempts,
           last_error,
           destination,
           source_aggregate_type,
           source_aggregate_id,
           source_sequence,
           correlation_id,
           causation_id
    FROM outbox_messages
    WHERE status = $1
    ORDER BY created_at ASC, message_id ASC
    "#
}

fn outbox_message_select_by_id_sql() -> &'static str {
    r#"
    SELECT message_id,
           event_type,
           payload,
           payload_codec,
           payload_codec_version,
           metadata::text AS metadata,
           status,
           EXTRACT(EPOCH FROM created_at)::double precision AS created_at_epoch,
           claimed_by,
           EXTRACT(EPOCH FROM claimed_until)::double precision AS claimed_until_epoch,
           attempts,
           last_error,
           destination,
           source_aggregate_type,
           source_aggregate_id,
           source_sequence,
           correlation_id,
           causation_id
    FROM outbox_messages
    WHERE message_id = $1
    "#
}

fn outbox_message_from_row(row: PgRow) -> Result<OutboxMessage, RepositoryError> {
    let status_text: String = row
        .try_get("status")
        .map_err(|err| repository_storage_error("decode outbox status row", err))?;
    let status = status_text.parse::<OutboxMessageStatus>().map_err(|_| {
        RepositoryError::Model(format!("postgres outbox status `{status_text}` is invalid"))
    })?;
    let metadata_json: String = row
        .try_get("metadata")
        .map_err(|err| repository_storage_error("decode outbox metadata row", err))?;
    let attempts: i32 = row
        .try_get("attempts")
        .map_err(|err| repository_storage_error("decode outbox attempts row", err))?;
    let source_sequence = row
        .try_get::<Option<i64>, _>("source_sequence")
        .map_err(|err| repository_storage_error("decode outbox source sequence row", err))?
        .map(|value| {
            sqlx_repository_u64_from_i64(POSTGRES_BACKEND, value, "outbox source sequence")
        })
        .transpose()?;

    Ok(OutboxMessage {
        id: row
            .try_get("message_id")
            .map_err(|err| repository_storage_error("decode outbox message id row", err))?,
        event_type: row
            .try_get("event_type")
            .map_err(|err| repository_storage_error("decode outbox event type row", err))?,
        payload: row
            .try_get("payload")
            .map_err(|err| repository_storage_error("decode outbox payload row", err))?,
        payload_codec: row
            .try_get("payload_codec")
            .map_err(|err| repository_storage_error("decode outbox payload codec row", err))?,
        payload_codec_version: sqlx_repository_u16_from_i32(
            POSTGRES_BACKEND,
            row.try_get("payload_codec_version").map_err(|err| {
                repository_storage_error("decode outbox payload codec version row", err)
            })?,
            "outbox payload codec version",
        )?,
        metadata: deserialize_event_metadata(&metadata_json)?,
        status,
        created_at: system_time_from_epoch_secs(
            row.try_get("created_at_epoch")
                .map_err(|err| repository_storage_error("decode outbox created_at row", err))?,
        )?,
        worker_id: row
            .try_get("claimed_by")
            .map_err(|err| repository_storage_error("decode outbox claimed_by row", err))?,
        leased_until: row
            .try_get::<Option<f64>, _>("claimed_until_epoch")
            .map_err(|err| repository_storage_error("decode outbox claimed_until row", err))?
            .map(system_time_from_epoch_secs)
            .transpose()?,
        attempts: u32::try_from(attempts).map_err(|_| {
            RepositoryError::Model(format!(
                "postgres outbox attempts value {attempts} is invalid"
            ))
        })?,
        last_error: row
            .try_get("last_error")
            .map_err(|err| repository_storage_error("decode outbox last_error row", err))?,
        destination: row
            .try_get("destination")
            .map_err(|err| repository_storage_error("decode outbox destination row", err))?,
        source_aggregate_type: row.try_get("source_aggregate_type").map_err(|err| {
            repository_storage_error("decode outbox source aggregate type row", err)
        })?,
        source_aggregate_id: row.try_get("source_aggregate_id").map_err(|err| {
            repository_storage_error("decode outbox source aggregate id row", err)
        })?,
        source_sequence,
    })
}

fn empty_string_as_none(value: &str) -> Option<&str> {
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

async fn ensure_outbox_update_applied(
    pool: &PgPool,
    rows_affected: u64,
    message_id: &str,
    validate: impl FnOnce(&OutboxMessage) -> Result<(), RepositoryError>,
) -> Result<(), RepositoryError> {
    if rows_affected > 0 {
        return Ok(());
    }

    let message = outbox_message_by_id_pool(pool, message_id)
        .await?
        .ok_or_else(|| RepositoryError::NotFound {
            id: message_id.to_string(),
        })?;
    validate(&message)
}

fn event_from_row(row: PgRow) -> Result<EventRecord, RepositoryError> {
    let payload_codec: String = row
        .try_get("payload_codec")
        .map_err(|err| repository_storage_error("decode payload codec row", err))?;
    let payload_codec_version = sqlx_repository_u16_from_i32(
        POSTGRES_BACKEND,
        row.try_get("payload_codec_version")
            .map_err(|err| repository_storage_error("decode payload codec version row", err))?,
        "payload_codec_version",
    )?;
    let metadata_json: String = row
        .try_get("metadata")
        .map_err(|err| repository_storage_error("decode metadata row", err))?;
    let metadata = deserialize_event_metadata(&metadata_json)?;
    let event = EventRecord {
        event_name: row
            .try_get("event_name")
            .map_err(|err| repository_storage_error("decode event name row", err))?,
        payload_codec,
        payload_codec_version,
        payload: row
            .try_get("payload")
            .map_err(|err| repository_storage_error("decode payload row", err))?,
        event_version: sqlx_repository_u64_from_i32(
            POSTGRES_BACKEND,
            row.try_get("event_version")
                .map_err(|err| repository_storage_error("decode event version row", err))?,
            "event_version",
        )?,
        sequence: sqlx_repository_u64_from_i64(
            POSTGRES_BACKEND,
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
    validate_snapshot_identity(identity, &record)?;

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
    .bind(sqlx_repository_i64_from_u64(
        POSTGRES_BACKEND,
        record.version,
        "snapshot version",
        BIGINT_STORAGE,
    )?)
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
        version: sqlx_repository_u64_from_i64(
            POSTGRES_BACKEND,
            row.try_get("version")
                .map_err(|err| repository_storage_error("decode snapshot version row", err))?,
            "snapshot version",
        )?,
        data: row
            .try_get("data")
            .map_err(|err| repository_storage_error("decode snapshot data row", err))?,
    })
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

fn repository_storage_error(operation: &str, err: sqlx::Error) -> RepositoryError {
    sqlx_repo::repository_storage_error(POSTGRES_BACKEND, operation, err)
}
