//! SQLite-backed async repository and transactional document stores.
//!
//! This adapter is a local SQL persistence backend for the async repository
//! boundary. It is feature-gated behind `sqlite` and is intentionally async-only.

#![expect(
    clippy::manual_async_fn,
    reason = "async trait impls return impl Future + Send to preserve public Send bounds"
)]

use std::collections::HashSet;
use std::future::Future;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sqlx::sqlite::SqlitePoolOptions;
use sqlx::{Row, Sqlite, SqlitePool, Transaction};

use crate::entity::{Entity, EventRecord};
use crate::outbox::{OutboxMessage, OutboxMessageStatus};
use crate::outbox_worker::{
    ensure_active_claim, AsyncOutboxStore, ClaimOutboxMessages, OutboxClaimRef,
    OutboxPublishFailureAction,
};
use crate::read_model::{
    ProcessedMessageMark, ReadModel, ReadModelAdapterCapabilities, ReadModelCommitOutcome,
    ReadModelError, ReadModelMutation, ReadModelWritePlan, Versioned,
};
use crate::repository::{
    AsyncCommitBatch, AsyncGetStream, AsyncReadModelSessionStore, AsyncReadModelStore,
    AsyncSnapshotStore, AsyncSnapshotWrite, AsyncTransactionalCommit, PreparedEventAppend,
    RepositoryError, StreamIdentity,
};
use crate::snapshot::SnapshotRecord;
use crate::sqlx_repo::{
    self, deserialize_event_metadata, is_sqlite_unique_constraint,
    read_model_i64_from_u64 as sqlx_read_model_i64_from_u64,
    read_model_u64_from_i64 as sqlx_read_model_u64_from_i64, reject_duplicate_outbox_messages,
    reject_duplicate_streams, repository_i64_from_u64 as sqlx_repository_i64_from_u64,
    repository_u16_from_i64 as sqlx_repository_u16_from_i64,
    repository_u64_from_i64 as sqlx_repository_u64_from_i64, serialize_event_metadata,
    validate_entity_id_matches_identity, validate_prepared_appends, validate_snapshot_identity,
    validate_supported_event_codec,
};
use crate::table::{
    generate_table_migration_artifacts, table_schema_bootstrap_result, table_schema_statements,
    TableMigrationArtifact, TableSchemaBootstrap, TableSchemaRegistry, TableSqlDialect,
    TableSqlSchemaAdapter, TableStoreError,
};

const SQLITE_SCHEMA: &str = include_str!("../../migrations/sqlite/0001_initial.sql");
const SQLITE_BACKEND: &str = "sqlite";
const SIGNED_INTEGER_STORAGE: &str = "signed integer storage";

/// SQLite-backed async repository.
#[derive(Clone)]
pub struct SqliteRepository {
    pool: SqlitePool,
}

/// SQLite-backed outbox table store.
#[derive(Clone)]
pub struct SqliteOutboxStore {
    pool: SqlitePool,
}

impl SqliteRepository {
    /// Create a repository from an existing migrated pool.
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Open a SQLite pool without applying migrations.
    pub async fn connect(database_url: &str) -> Result<Self, RepositoryError> {
        let pool = SqlitePoolOptions::new()
            .max_connections(default_pool_size(database_url))
            .connect(database_url)
            .await
            .map_err(|err| repository_storage_error("connect", err))?;
        Ok(Self::new(pool))
    }

    /// Open a SQLite pool and apply the explicit SQLite migrations.
    pub async fn connect_and_migrate(database_url: &str) -> Result<Self, RepositoryError> {
        let repo = Self::connect(database_url).await?;
        repo.migrate().await?;
        Ok(repo)
    }

    /// Apply SQLite migrations to this repository's pool.
    pub async fn migrate(&self) -> Result<(), RepositoryError> {
        Self::migrate_pool(&self.pool).await
    }

    /// Apply SQLite migrations to an existing pool.
    pub async fn migrate_pool(pool: &SqlitePool) -> Result<(), RepositoryError> {
        for statement in SQLITE_SCHEMA.split(';') {
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
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// SQL artifact adapter for registered table/read-model schemas.
    pub fn table_schema_adapter(&self) -> TableSqlSchemaAdapter {
        TableSqlSchemaAdapter::sqlite()
    }

    /// Generate SQL statements for registered table/read-model schemas.
    pub fn generate_table_migration_artifacts(
        &self,
        registry: &TableSchemaRegistry,
    ) -> Result<Vec<TableMigrationArtifact>, TableStoreError> {
        generate_table_migration_artifacts(registry, TableSqlDialect::Sqlite)
    }

    /// Explicit dev/test bootstrap for registered table/read-model schemas.
    pub async fn bootstrap_table_schema_for_dev(
        &self,
        registry: &TableSchemaRegistry,
    ) -> Result<TableSchemaBootstrap, TableStoreError> {
        for statement in table_schema_statements(registry, TableSqlDialect::Sqlite)? {
            sqlx::query(&statement)
                .execute(&self.pool)
                .await
                .map_err(|err| table_schema_storage_error("bootstrap table schema", err))?;
        }
        Ok(table_schema_bootstrap_result(registry))
    }

    /// Access an outbox-store handle backed by this repository's pool.
    pub fn outbox_store(&self) -> SqliteOutboxStore {
        SqliteOutboxStore {
            pool: self.pool.clone(),
        }
    }
}

impl SqliteOutboxStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// SQL artifact adapter for registered table/read-model schemas.
    pub fn table_schema_adapter(&self) -> TableSqlSchemaAdapter {
        TableSqlSchemaAdapter::sqlite()
    }

    /// Generate SQL statements for registered table/read-model schemas.
    pub fn generate_table_migration_artifacts(
        &self,
        registry: &TableSchemaRegistry,
    ) -> Result<Vec<TableMigrationArtifact>, TableStoreError> {
        generate_table_migration_artifacts(registry, TableSqlDialect::Sqlite)
    }

    /// Explicit dev/test bootstrap for registered table/read-model schemas.
    pub async fn bootstrap_table_schema_for_dev(
        &self,
        registry: &TableSchemaRegistry,
    ) -> Result<TableSchemaBootstrap, TableStoreError> {
        for statement in table_schema_statements(registry, TableSqlDialect::Sqlite)? {
            sqlx::query(&statement)
                .execute(&self.pool)
                .await
                .map_err(|err| table_schema_storage_error("bootstrap table schema", err))?;
        }
        Ok(table_schema_bootstrap_result(registry))
    }
}

impl AsyncGetStream for SqliteRepository {
    fn get_stream<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<Option<Entity>, RepositoryError>> + Send + 'a {
        async move {
            let rows = sqlx::query(
                r#"
                SELECT event_name, event_version, payload, payload_codec,
                       payload_codec_version, metadata, sequence, recorded_at
                FROM aggregate_events
                WHERE aggregate_type = ? AND aggregate_id = ?
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

impl AsyncTransactionalCommit for SqliteRepository {
    fn commit_batch_async<'a>(
        &'a self,
        batch: AsyncCommitBatch<'a>,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        async move {
            reject_duplicate_streams(&batch.streams)?;
            reject_duplicate_outbox_messages(&batch.outbox_messages)?;
            validate_entity_id_matches_identity(&batch.streams)?;

            let prepared = batch
                .streams
                .iter()
                .map(PreparedEventAppend::from_stream_write)
                .collect::<Vec<_>>();
            validate_prepared_appends(&prepared)?;

            for plan in &batch.read_model_plans {
                validate_document_write_plan(plan)?;
            }

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
                    insert_event_in_tx(&mut tx, &append.identity, append.expected_version, event)
                        .await?;
                }
            }

            for message in &batch.outbox_messages {
                insert_outbox_message_in_tx(&mut tx, message).await?;
            }

            for plan in batch.read_model_plans {
                let outcome = apply_document_write_plan_in_tx(&mut tx, plan).await?;
                if let Some(mark) = outcome.duplicate_message() {
                    return Err(RepositoryError::Model(format!(
                        "processed message already handled by consumer `{}`: `{}`",
                        mark.consumer_name, mark.message_id
                    )));
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

impl AsyncReadModelStore for SqliteRepository {
    fn get_model_async<'a, M>(
        &'a self,
        id: &'a str,
    ) -> impl Future<Output = Result<Option<Versioned<M>>, ReadModelError>> + Send + 'a
    where
        M: ReadModel + 'a,
    {
        async move { self.load_document_model::<M>(id).await }
    }

    fn get_by_primary_key_async<'a, M>(
        &'a self,
        id: &'a str,
    ) -> impl Future<Output = Result<Option<Versioned<M>>, ReadModelError>> + Send + 'a
    where
        M: ReadModel + 'a,
    {
        async move { self.load_document_model::<M>(id).await }
    }

    fn upsert_async<'a, M>(
        &'a self,
        model: &'a M,
    ) -> impl Future<Output = Result<Versioned<M>, ReadModelError>> + Send + 'a
    where
        M: ReadModel + 'a,
    {
        async move {
            let bytes =
                serde_json::to_vec(model).map_err(|err| ReadModelError::Serde(err.to_string()))?;
            let mut tx = begin_read_model_tx(&self.pool).await?;
            let version = upsert_document_in_tx(&mut tx, M::COLLECTION, model.id(), bytes).await?;
            commit_read_model_tx(tx).await?;
            Ok(Versioned {
                data: model.clone(),
                version,
            })
        }
    }

    fn insert_async<'a, M>(
        &'a self,
        model: &'a M,
    ) -> impl Future<Output = Result<Versioned<M>, ReadModelError>> + Send + 'a
    where
        M: ReadModel + 'a,
    {
        async move {
            let bytes =
                serde_json::to_vec(model).map_err(|err| ReadModelError::Serde(err.to_string()))?;
            let mut tx = begin_read_model_tx(&self.pool).await?;
            let existing = document_version_in_tx(&mut tx, M::COLLECTION, model.id()).await?;
            if let Some(actual) = existing {
                return Err(ReadModelError::ConcurrencyConflict {
                    collection: M::COLLECTION.to_string(),
                    id: model.id().to_string(),
                    expected: 0,
                    actual,
                });
            }
            insert_document_in_tx(&mut tx, M::COLLECTION, model.id(), bytes, 1).await?;
            commit_read_model_tx(tx).await?;
            Ok(Versioned {
                data: model.clone(),
                version: 1,
            })
        }
    }

    fn update_async<'a, M>(
        &'a self,
        model: &'a M,
        expected_version: u64,
    ) -> impl Future<Output = Result<Versioned<M>, ReadModelError>> + Send + 'a
    where
        M: ReadModel + 'a,
    {
        async move {
            let bytes =
                serde_json::to_vec(model).map_err(|err| ReadModelError::Serde(err.to_string()))?;
            let mut tx = begin_read_model_tx(&self.pool).await?;
            let actual = document_version_in_tx(&mut tx, M::COLLECTION, model.id())
                .await?
                .ok_or_else(|| ReadModelError::NotFound {
                    collection: M::COLLECTION.to_string(),
                    id: model.id().to_string(),
                })?;
            if actual != expected_version {
                return Err(ReadModelError::ConcurrencyConflict {
                    collection: M::COLLECTION.to_string(),
                    id: model.id().to_string(),
                    expected: expected_version,
                    actual,
                });
            }
            let new_version = next_document_version(M::COLLECTION, model.id(), Some(actual))?;
            let rows_affected = update_document_in_tx(
                &mut tx,
                M::COLLECTION,
                model.id(),
                bytes,
                actual,
                new_version,
            )
            .await?;
            if rows_affected == 0 {
                let current = document_version_in_tx(&mut tx, M::COLLECTION, model.id())
                    .await?
                    .unwrap_or(actual);
                return Err(ReadModelError::ConcurrencyConflict {
                    collection: M::COLLECTION.to_string(),
                    id: model.id().to_string(),
                    expected: expected_version,
                    actual: current,
                });
            }
            commit_read_model_tx(tx).await?;
            Ok(Versioned {
                data: model.clone(),
                version: new_version,
            })
        }
    }

    fn delete_async<'a, M>(
        &'a self,
        id: &'a str,
    ) -> impl Future<Output = Result<bool, ReadModelError>> + Send + 'a
    where
        M: ReadModel + 'a,
    {
        async move {
            let result = sqlx::query(
                r#"
                DELETE FROM transactional_read_models
                WHERE collection = ? AND id = ?
                "#,
            )
            .bind(M::COLLECTION)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|err| read_model_storage_error("delete document", err))?;

            Ok(result.rows_affected() > 0)
        }
    }
}

impl SqliteRepository {
    async fn load_document_model<M: ReadModel>(
        &self,
        id: &str,
    ) -> Result<Option<Versioned<M>>, ReadModelError> {
        let row = sqlx::query(
            r#"
            SELECT payload, version
            FROM transactional_read_models
            WHERE collection = ? AND id = ?
            "#,
        )
        .bind(M::COLLECTION)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| read_model_storage_error("load document", err))?;

        let Some(row) = row else {
            return Ok(None);
        };

        let payload: Vec<u8> = row
            .try_get("payload")
            .map_err(|err| read_model_storage_error("decode document payload row", err))?;
        let version = sqlx_read_model_u64_from_i64(
            SQLITE_BACKEND,
            row.try_get("version")
                .map_err(|err| read_model_storage_error("decode document version row", err))?,
            "version",
        )?;
        let data = serde_json::from_slice(&payload)
            .map_err(|err| ReadModelError::Serde(err.to_string()))?;

        Ok(Some(Versioned { data, version }))
    }
}

impl AsyncReadModelSessionStore for SqliteRepository {
    fn read_model_capabilities_async(&self) -> ReadModelAdapterCapabilities {
        document_capabilities()
    }

    fn commit_write_plan_async(
        &self,
        plan: ReadModelWritePlan,
    ) -> impl Future<Output = Result<ReadModelCommitOutcome, ReadModelError>> + Send + '_ {
        async move {
            validate_document_write_plan(&plan)?;
            let mut tx = begin_read_model_tx(&self.pool).await?;
            let outcome = apply_document_write_plan_in_tx(&mut tx, plan).await?;
            if outcome.was_applied() {
                commit_read_model_tx(tx).await?;
            }
            Ok(outcome)
        }
    }

    fn is_processed_async<'a>(
        &'a self,
        consumer_name: &'a str,
        message_id: &'a str,
    ) -> impl Future<Output = Result<bool, ReadModelError>> + Send + 'a {
        async move { processed_message_exists_pool(&self.pool, consumer_name, message_id).await }
    }
}

impl AsyncOutboxStore for SqliteOutboxStore {
    fn messages_by_status_async(
        &self,
        status: OutboxMessageStatus,
    ) -> impl Future<Output = Result<Vec<OutboxMessage>, RepositoryError>> + Send + '_ {
        async move {
            let rows = sqlx::query(
                r#"
                SELECT message_id, event_type, payload, payload_codec, payload_codec_version,
                       metadata, status, created_at,
                       claimed_by, claimed_until, attempts, last_error, destination,
                       source_aggregate_type, source_aggregate_id, source_sequence,
                       correlation_id, causation_id
                FROM outbox_messages
                WHERE status = ?
                ORDER BY CAST(created_at AS REAL) ASC, message_id ASC
                "#,
            )
            .bind(status.as_str())
            .fetch_all(&self.pool)
            .await
            .map_err(|err| repository_storage_error("load outbox messages by status", err))?;

            rows.into_iter().map(outbox_message_from_row).collect()
        }
    }

    fn claim_async<'a>(
        &'a self,
        request: ClaimOutboxMessages,
    ) -> impl Future<Output = Result<Vec<OutboxMessage>, RepositoryError>> + Send + 'a {
        async move {
            if request.batch_size == 0 {
                return Ok(Vec::new());
            }

            let now = SystemTime::now();
            let now_epoch = system_time_to_epoch_secs(now)?;
            let claimed_until = now.checked_add(request.lease).ok_or_else(|| {
                RepositoryError::Model("failed to compute outbox lease deadline".into())
            })?;
            let claimed_until_storage = system_time_to_storage(claimed_until)?;

            let mut tx =
                self.pool.begin().await.map_err(|err| {
                    repository_storage_error("begin outbox claim transaction", err)
                })?;

            let limit = sqlx_repository_i64_from_u64(
                SQLITE_BACKEND,
                request.batch_size as u64,
                "outbox claim limit",
                SIGNED_INTEGER_STORAGE,
            )?;
            let candidate_rows = sqlx::query(
                r#"
                SELECT message_id
                FROM outbox_messages
                WHERE (
                    (status = ? AND CAST(next_available_at AS REAL) <= ?)
                    OR (status = ? AND (claimed_until IS NULL OR CAST(claimed_until AS REAL) <= ?))
                )
                  AND (? IS NULL OR destination = ?)
                ORDER BY CAST(created_at AS REAL) ASC, message_id ASC
                LIMIT ?
                "#,
            )
            .bind(OutboxMessageStatus::Pending.as_str())
            .bind(now_epoch)
            .bind(OutboxMessageStatus::InFlight.as_str())
            .bind(now_epoch)
            .bind(request.destination.as_deref())
            .bind(request.destination.as_deref())
            .bind(limit)
            .fetch_all(&mut *tx)
            .await
            .map_err(|err| repository_storage_error("select claimable outbox messages", err))?;

            let mut claimed = Vec::new();
            for row in candidate_rows {
                let message_id: String = row
                    .try_get("message_id")
                    .map_err(|err| repository_storage_error("decode outbox message id row", err))?;
                let result = sqlx::query(
                    r#"
                    UPDATE outbox_messages
                    SET status = ?,
                        claimed_by = ?,
                        claimed_until = ?,
                        attempts = attempts + 1,
                        updated_at = CURRENT_TIMESTAMP
                    WHERE message_id = ?
                      AND (
                        (status = ? AND CAST(next_available_at AS REAL) <= ?)
                        OR (
                          status = ?
                          AND (claimed_until IS NULL OR CAST(claimed_until AS REAL) <= ?)
                        )
                      )
                      AND (? IS NULL OR destination = ?)
                    "#,
                )
                .bind(OutboxMessageStatus::InFlight.as_str())
                .bind(&request.worker_id)
                .bind(&claimed_until_storage)
                .bind(&message_id)
                .bind(OutboxMessageStatus::Pending.as_str())
                .bind(now_epoch)
                .bind(OutboxMessageStatus::InFlight.as_str())
                .bind(now_epoch)
                .bind(request.destination.as_deref())
                .bind(request.destination.as_deref())
                .execute(&mut *tx)
                .await
                .map_err(|err| repository_storage_error("claim outbox message", err))?;

                if result.rows_affected() == 0 {
                    continue;
                }

                if let Some(message) = outbox_message_by_id_in_tx(&mut tx, &message_id).await? {
                    claimed.push(message);
                }
            }

            tx.commit()
                .await
                .map_err(|err| repository_storage_error("commit outbox claim transaction", err))?;
            Ok(claimed)
        }
    }

    fn complete_async<'a>(
        &'a self,
        claim: &'a OutboxClaimRef,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        async move {
            let now = SystemTime::now();
            let now_epoch = system_time_to_epoch_secs(now)?;
            let result = sqlx::query(
                r#"
                UPDATE outbox_messages
                SET status = ?,
                    claimed_by = NULL,
                    claimed_until = NULL,
                    published_at = ?,
                    updated_at = CURRENT_TIMESTAMP
                WHERE message_id = ?
                  AND status = ?
                  AND claimed_by = ?
                  AND claimed_until IS NOT NULL
                  AND CAST(claimed_until AS REAL) > ?
                  AND attempts = ?
                "#,
            )
            .bind(OutboxMessageStatus::Published.as_str())
            .bind(system_time_to_storage(now)?)
            .bind(&claim.message_id)
            .bind(OutboxMessageStatus::InFlight.as_str())
            .bind(&claim.worker_id)
            .bind(now_epoch)
            .bind(sqlx_repository_i64_from_u64(
                SQLITE_BACKEND,
                u64::from(claim.attempt),
                "outbox claim attempt",
                SIGNED_INTEGER_STORAGE,
            )?)
            .execute(&self.pool)
            .await
            .map_err(|err| repository_storage_error("complete outbox message", err))?;

            ensure_outbox_update_applied(
                &self.pool,
                result.rows_affected(),
                &claim.message_id,
                |message| ensure_active_claim(message, Some(claim), now),
            )
            .await
        }
    }

    fn release_async<'a>(
        &'a self,
        claim: &'a OutboxClaimRef,
        error: &'a str,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        async move {
            let now = SystemTime::now();
            let now_epoch = system_time_to_epoch_secs(now)?;
            let now_storage = system_time_to_storage(now)?;
            let result = sqlx::query(
                r#"
                UPDATE outbox_messages
                SET status = ?,
                    claimed_by = NULL,
                    claimed_until = NULL,
                    next_available_at = ?,
                    last_error = ?,
                    updated_at = CURRENT_TIMESTAMP
                WHERE message_id = ?
                  AND status = ?
                  AND claimed_by = ?
                  AND claimed_until IS NOT NULL
                  AND CAST(claimed_until AS REAL) > ?
                  AND attempts = ?
                "#,
            )
            .bind(OutboxMessageStatus::Pending.as_str())
            .bind(now_storage)
            .bind(empty_string_as_none(error))
            .bind(&claim.message_id)
            .bind(OutboxMessageStatus::InFlight.as_str())
            .bind(&claim.worker_id)
            .bind(now_epoch)
            .bind(sqlx_repository_i64_from_u64(
                SQLITE_BACKEND,
                u64::from(claim.attempt),
                "outbox claim attempt",
                SIGNED_INTEGER_STORAGE,
            )?)
            .execute(&self.pool)
            .await
            .map_err(|err| repository_storage_error("release outbox message", err))?;

            ensure_outbox_update_applied(
                &self.pool,
                result.rows_affected(),
                &claim.message_id,
                |message| ensure_active_claim(message, Some(claim), now),
            )
            .await
        }
    }

    fn fail_async<'a>(
        &'a self,
        claim: &'a OutboxClaimRef,
        error: &'a str,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        async move {
            let now = SystemTime::now();
            let now_epoch = system_time_to_epoch_secs(now)?;
            let result = sqlx::query(
                r#"
                UPDATE outbox_messages
                SET status = ?,
                    claimed_by = NULL,
                    claimed_until = NULL,
                    last_error = ?,
                    failed_at = ?,
                    updated_at = CURRENT_TIMESTAMP
                WHERE message_id = ?
                  AND status = ?
                  AND claimed_by = ?
                  AND claimed_until IS NOT NULL
                  AND CAST(claimed_until AS REAL) > ?
                  AND attempts = ?
                "#,
            )
            .bind(OutboxMessageStatus::Failed.as_str())
            .bind(empty_string_as_none(error))
            .bind(system_time_to_storage(now)?)
            .bind(&claim.message_id)
            .bind(OutboxMessageStatus::InFlight.as_str())
            .bind(&claim.worker_id)
            .bind(now_epoch)
            .bind(sqlx_repository_i64_from_u64(
                SQLITE_BACKEND,
                u64::from(claim.attempt),
                "outbox claim attempt",
                SIGNED_INTEGER_STORAGE,
            )?)
            .execute(&self.pool)
            .await
            .map_err(|err| repository_storage_error("fail outbox message", err))?;

            ensure_outbox_update_applied(
                &self.pool,
                result.rows_affected(),
                &claim.message_id,
                |message| ensure_active_claim(message, Some(claim), now),
            )
            .await
        }
    }

    fn record_failure_async<'a>(
        &'a self,
        claim: &'a OutboxClaimRef,
        error: &'a str,
        max_attempts: u32,
    ) -> impl Future<Output = Result<OutboxPublishFailureAction, RepositoryError>> + Send + 'a {
        async move {
            let message = outbox_message_by_id_pool(&self.pool, &claim.message_id)
                .await?
                .ok_or_else(|| RepositoryError::NotFound {
                    id: claim.message_id.clone(),
                })?;
            ensure_active_claim(&message, Some(claim), SystemTime::now())?;

            if message.attempts >= max_attempts {
                self.fail_async(claim, error).await?;
                Ok(OutboxPublishFailureAction::Failed)
            } else {
                self.release_async(claim, error).await?;
                Ok(OutboxPublishFailureAction::Released)
            }
        }
    }
}

impl AsyncSnapshotStore for SqliteRepository {
    fn get_snapshot_async<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<Option<SnapshotRecord>, RepositoryError>> + Send + 'a {
        async move {
            let row = sqlx::query(
                r#"
                SELECT aggregate_type, aggregate_id, version, snapshot_type,
                       snapshot_version, payload, payload_codec,
                       payload_codec_version, metadata, recorded_at
                FROM aggregate_snapshots
                WHERE aggregate_type = ? AND aggregate_id = ?
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
                WHERE aggregate_type = ? AND aggregate_id = ?
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

fn empty_string_as_none(value: &str) -> Option<&str> {
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

async fn insert_outbox_message_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    message: &OutboxMessage,
) -> Result<(), RepositoryError> {
    let metadata = serialize_event_metadata(&message.metadata)?;
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
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(message.id())
    .bind(&message.event_type)
    .bind(&message.payload)
    .bind(&message.payload_codec)
    .bind(i64::from(message.payload_codec_version))
    .bind(&message.destination)
    .bind(metadata)
    .bind(message.status.as_str())
    .bind(system_time_to_storage(message.created_at)?)
    .bind(system_time_to_storage(message.created_at)?)
    .bind(&message.worker_id)
    .bind(
        message
            .leased_until
            .map(system_time_to_storage)
            .transpose()?,
    )
    .bind(i64::from(message.attempts))
    .bind(&message.last_error)
    .bind(&message.source_aggregate_type)
    .bind(&message.source_aggregate_id)
    .bind(
        message
            .source_sequence
            .map(|value| {
                sqlx_repository_i64_from_u64(
                    SQLITE_BACKEND,
                    value,
                    "outbox source sequence",
                    SIGNED_INTEGER_STORAGE,
                )
            })
            .transpose()?,
    )
    .bind(message.correlation_id())
    .bind(message.causation_id())
    .execute(&mut **tx)
    .await;

    match result {
        Ok(_) => Ok(()),
        Err(err) if is_sqlite_unique_constraint(&err) => {
            Err(RepositoryError::DuplicateOutboxMessageInBatch {
                id: message.id().to_string(),
            })
        }
        Err(err) => Err(repository_storage_error("insert outbox message", err)),
    }
}

async fn outbox_message_by_id_pool(
    pool: &SqlitePool,
    message_id: &str,
) -> Result<Option<OutboxMessage>, RepositoryError> {
    let row = sqlx::query(outbox_message_select_sql())
        .bind(message_id)
        .fetch_optional(pool)
        .await
        .map_err(|err| repository_storage_error("load outbox message", err))?;
    row.map(outbox_message_from_row).transpose()
}

async fn outbox_message_by_id_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    message_id: &str,
) -> Result<Option<OutboxMessage>, RepositoryError> {
    let row = sqlx::query(outbox_message_select_sql())
        .bind(message_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|err| repository_storage_error("load outbox message", err))?;
    row.map(outbox_message_from_row).transpose()
}

fn outbox_message_select_sql() -> &'static str {
    r#"
    SELECT message_id, event_type, payload, payload_codec, payload_codec_version,
           metadata, status, created_at,
           claimed_by, claimed_until, attempts, last_error, destination,
           source_aggregate_type, source_aggregate_id, source_sequence,
           correlation_id, causation_id
    FROM outbox_messages
    WHERE message_id = ?
    "#
}

fn outbox_message_from_row(row: sqlx::sqlite::SqliteRow) -> Result<OutboxMessage, RepositoryError> {
    let status_text: String = row
        .try_get("status")
        .map_err(|err| repository_storage_error("decode outbox status row", err))?;
    let status = status_text.parse::<OutboxMessageStatus>().map_err(|_| {
        RepositoryError::Model(format!("sqlite outbox status `{status_text}` is invalid"))
    })?;
    let metadata_json: String = row
        .try_get("metadata")
        .map_err(|err| repository_storage_error("decode outbox metadata row", err))?;
    let mut message = OutboxMessage::new();
    let message_id: String = row
        .try_get("message_id")
        .map_err(|err| repository_storage_error("decode outbox message id row", err))?;
    message.id = message_id;
    message.event_type = row
        .try_get("event_type")
        .map_err(|err| repository_storage_error("decode outbox event type row", err))?;
    message.payload = row
        .try_get("payload")
        .map_err(|err| repository_storage_error("decode outbox payload row", err))?;
    message.payload_codec = row
        .try_get("payload_codec")
        .map_err(|err| repository_storage_error("decode outbox payload codec row", err))?;
    let payload_codec_version: i64 = row
        .try_get("payload_codec_version")
        .map_err(|err| repository_storage_error("decode outbox payload codec version row", err))?;
    message.payload_codec_version = sqlx_repository_u16_from_i64(
        SQLITE_BACKEND,
        payload_codec_version,
        "outbox payload codec version",
    )?;
    message.metadata = deserialize_event_metadata(&metadata_json)?;
    message.status = status;
    message.created_at = system_time_from_storage(
        row.try_get::<String, _>("created_at")
            .map_err(|err| repository_storage_error("decode outbox created_at row", err))?
            .as_str(),
    );
    message.worker_id = row
        .try_get("claimed_by")
        .map_err(|err| repository_storage_error("decode outbox claimed_by row", err))?;
    message.leased_until = row
        .try_get::<Option<String>, _>("claimed_until")
        .map_err(|err| repository_storage_error("decode outbox claimed_until row", err))?
        .as_deref()
        .map(system_time_from_storage);
    let attempts: i64 = row
        .try_get("attempts")
        .map_err(|err| repository_storage_error("decode outbox attempts row", err))?;
    message.attempts = u32::try_from(attempts).map_err(|_| {
        RepositoryError::Model(format!(
            "sqlite outbox attempts value {attempts} is invalid"
        ))
    })?;
    message.last_error = row
        .try_get("last_error")
        .map_err(|err| repository_storage_error("decode outbox last_error row", err))?;
    message.destination = row
        .try_get("destination")
        .map_err(|err| repository_storage_error("decode outbox destination row", err))?;
    message.source_aggregate_type = row
        .try_get("source_aggregate_type")
        .map_err(|err| repository_storage_error("decode outbox source aggregate type row", err))?;
    message.source_aggregate_id = row
        .try_get("source_aggregate_id")
        .map_err(|err| repository_storage_error("decode outbox source aggregate id row", err))?;
    message.source_sequence = row
        .try_get::<Option<i64>, _>("source_sequence")
        .map_err(|err| repository_storage_error("decode outbox source sequence row", err))?
        .map(|value| sqlx_repository_u64_from_i64(SQLITE_BACKEND, value, "outbox source sequence"))
        .transpose()?;
    if let Some(correlation_id) = row
        .try_get::<Option<String>, _>("correlation_id")
        .map_err(|err| repository_storage_error("decode outbox correlation_id row", err))?
    {
        message.set_correlation_id(correlation_id);
    }
    if let Some(causation_id) = row
        .try_get::<Option<String>, _>("causation_id")
        .map_err(|err| repository_storage_error("decode outbox causation_id row", err))?
    {
        message.set_causation_id(causation_id);
    }
    Ok(message)
}

async fn ensure_outbox_update_applied(
    pool: &SqlitePool,
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

fn default_pool_size(database_url: &str) -> u32 {
    if database_url.contains(":memory:") {
        1
    } else {
        5
    }
}

async fn stream_version_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    identity: &StreamIdentity,
) -> Result<u64, RepositoryError> {
    let row = sqlx::query(
        r#"
        SELECT MAX(sequence) AS version
        FROM aggregate_events
        WHERE aggregate_type = ? AND aggregate_id = ?
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
        .map(|value| sqlx_repository_u64_from_i64(SQLITE_BACKEND, value, "sequence"))
        .unwrap_or(Ok(0))
}

async fn insert_event_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
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
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(identity.aggregate_type())
    .bind(identity.aggregate_id())
    .bind(sqlx_repository_i64_from_u64(
        SQLITE_BACKEND,
        event.sequence,
        "sequence",
        SIGNED_INTEGER_STORAGE,
    )?)
    .bind(&event.event_name)
    .bind(sqlx_repository_i64_from_u64(
        SQLITE_BACKEND,
        event.event_version,
        "event_version",
        SIGNED_INTEGER_STORAGE,
    )?)
    .bind(&event.payload)
    .bind(&event.payload_codec)
    .bind(i64::from(event.payload_codec_version))
    .bind(metadata)
    .bind(system_time_to_storage(event.timestamp)?)
    .execute(&mut **tx)
    .await;

    match result {
        Ok(_) => Ok(()),
        Err(err) if is_sqlite_unique_constraint(&err) => {
            let actual = stream_version_in_tx(tx, identity).await?;
            Err(RepositoryError::ConcurrentWrite {
                id: identity.to_string(),
                expected: expected_version,
                actual,
            })
        }
        Err(err) => Err(repository_storage_error("insert event", err)),
    }
}

fn event_from_row(row: sqlx::sqlite::SqliteRow) -> Result<EventRecord, RepositoryError> {
    let payload_codec: String = row
        .try_get("payload_codec")
        .map_err(|err| repository_storage_error("decode payload codec row", err))?;
    let payload_codec_version = sqlx_repository_u16_from_i64(
        SQLITE_BACKEND,
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
        event_version: sqlx_repository_u64_from_i64(
            SQLITE_BACKEND,
            row.try_get("event_version")
                .map_err(|err| repository_storage_error("decode event version row", err))?,
            "event_version",
        )?,
        sequence: sqlx_repository_u64_from_i64(
            SQLITE_BACKEND,
            row.try_get("sequence")
                .map_err(|err| repository_storage_error("decode sequence row", err))?,
            "sequence",
        )?,
        timestamp: system_time_from_storage(
            row.try_get::<String, _>("recorded_at")
                .map_err(|err| repository_storage_error("decode recorded_at row", err))?
                .as_str(),
        ),
        metadata,
    };
    validate_supported_event_codec(&event)?;
    Ok(event)
}

fn document_capabilities() -> ReadModelAdapterCapabilities {
    ReadModelAdapterCapabilities {
        relational_rows: false,
        document_rows: true,
        sparse_patches: false,
        deletes: false,
        processed_messages: true,
    }
}

fn validate_document_write_plan(plan: &ReadModelWritePlan) -> Result<(), ReadModelError> {
    for mutation in &plan.mutations {
        if !matches!(mutation, ReadModelMutation::Document(_)) {
            return Err(ReadModelError::Metadata(
                "SqliteRepository currently supports only document read-model mutations".into(),
            ));
        }
    }
    plan.validate_for(&document_capabilities())
}

async fn begin_read_model_tx(pool: &SqlitePool) -> Result<Transaction<'_, Sqlite>, ReadModelError> {
    pool.begin()
        .await
        .map_err(|err| read_model_storage_error("begin transaction", err))
}

async fn commit_read_model_tx(tx: Transaction<'_, Sqlite>) -> Result<(), ReadModelError> {
    tx.commit()
        .await
        .map_err(|err| read_model_storage_error("commit transaction", err))
}

async fn apply_document_write_plan_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    plan: ReadModelWritePlan,
) -> Result<ReadModelCommitOutcome, ReadModelError> {
    validate_document_write_plan(&plan)?;

    let mut marks_in_plan = HashSet::with_capacity(plan.processed_messages.len());
    for mark in &plan.processed_messages {
        let key = processed_message_key(mark);
        if !marks_in_plan.insert(key) || processed_message_exists_in_tx(tx, mark).await? {
            return Ok(ReadModelCommitOutcome::skipped_duplicate(mark.clone()));
        }
    }

    for mutation in plan.mutations {
        match mutation {
            ReadModelMutation::Document(mutation) => {
                upsert_document_in_tx(tx, &mutation.collection, &mutation.id, mutation.bytes)
                    .await?;
            }
            _ => {
                return Err(ReadModelError::Metadata(
                    "SqliteRepository currently supports only document read-model mutations".into(),
                ));
            }
        }
    }

    for mark in plan.processed_messages {
        let result = insert_processed_message_in_tx(tx, &mark).await;
        if let Err(err) = result {
            if is_sqlite_unique_constraint(&err) {
                return Ok(ReadModelCommitOutcome::skipped_duplicate(mark));
            }
            return Err(read_model_storage_error("insert processed message", err));
        }
    }

    Ok(ReadModelCommitOutcome::applied())
}

async fn upsert_document_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    collection: &str,
    id: &str,
    bytes: Vec<u8>,
) -> Result<u64, ReadModelError> {
    let current = document_version_in_tx(tx, collection, id).await?;
    let new_version = next_document_version(collection, id, current)?;
    match current {
        Some(expected_version) => {
            let rows_affected =
                update_document_in_tx(tx, collection, id, bytes, expected_version, new_version)
                    .await?;
            if rows_affected == 0 {
                let actual = document_version_in_tx(tx, collection, id)
                    .await?
                    .unwrap_or(expected_version);
                return Err(ReadModelError::ConcurrencyConflict {
                    collection: collection.to_string(),
                    id: id.to_string(),
                    expected: expected_version,
                    actual,
                });
            }
        }
        None => insert_document_in_tx(tx, collection, id, bytes, new_version).await?,
    }
    Ok(new_version)
}

async fn document_version_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    collection: &str,
    id: &str,
) -> Result<Option<u64>, ReadModelError> {
    let row = sqlx::query(
        r#"
        SELECT version
        FROM transactional_read_models
        WHERE collection = ? AND id = ?
        "#,
    )
    .bind(collection)
    .bind(id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| read_model_storage_error("load document version", err))?;

    row.map(|row| {
        sqlx_read_model_u64_from_i64(
            SQLITE_BACKEND,
            row.try_get("version")
                .map_err(|err| read_model_storage_error("decode document version row", err))?,
            "version",
        )
    })
    .transpose()
}

async fn insert_document_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    collection: &str,
    id: &str,
    bytes: Vec<u8>,
    version: u64,
) -> Result<(), ReadModelError> {
    sqlx::query(
        r#"
        INSERT INTO transactional_read_models (collection, id, version, payload)
        VALUES (?, ?, ?, ?)
        "#,
    )
    .bind(collection)
    .bind(id)
    .bind(sqlx_read_model_i64_from_u64(
        SQLITE_BACKEND,
        version,
        "version",
        SIGNED_INTEGER_STORAGE,
    )?)
    .bind(bytes)
    .execute(&mut **tx)
    .await
    .map_err(|err| read_model_storage_error("insert document", err))?;

    Ok(())
}

async fn update_document_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    collection: &str,
    id: &str,
    bytes: Vec<u8>,
    expected_version: u64,
    version: u64,
) -> Result<u64, ReadModelError> {
    let result = sqlx::query(
        r#"
        UPDATE transactional_read_models
        SET version = ?, payload = ?, updated_at = CURRENT_TIMESTAMP
        WHERE collection = ? AND id = ? AND version = ?
        "#,
    )
    .bind(sqlx_read_model_i64_from_u64(
        SQLITE_BACKEND,
        version,
        "version",
        SIGNED_INTEGER_STORAGE,
    )?)
    .bind(bytes)
    .bind(collection)
    .bind(id)
    .bind(sqlx_read_model_i64_from_u64(
        SQLITE_BACKEND,
        expected_version,
        "expected version",
        SIGNED_INTEGER_STORAGE,
    )?)
    .execute(&mut **tx)
    .await
    .map_err(|err| read_model_storage_error("update document", err))?;

    Ok(result.rows_affected())
}

async fn processed_message_exists_pool(
    pool: &SqlitePool,
    consumer_name: &str,
    message_id: &str,
) -> Result<bool, ReadModelError> {
    let row = sqlx::query(
        r#"
        SELECT 1
        FROM read_model_processed_messages
        WHERE consumer_name = ? AND message_id = ?
        "#,
    )
    .bind(consumer_name)
    .bind(message_id)
    .fetch_optional(pool)
    .await
    .map_err(|err| read_model_storage_error("load processed message", err))?;
    Ok(row.is_some())
}

async fn processed_message_exists_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    mark: &ProcessedMessageMark,
) -> Result<bool, ReadModelError> {
    let row = sqlx::query(
        r#"
        SELECT 1
        FROM read_model_processed_messages
        WHERE consumer_name = ? AND message_id = ?
        "#,
    )
    .bind(&mark.consumer_name)
    .bind(&mark.message_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| read_model_storage_error("load processed message", err))?;
    Ok(row.is_some())
}

async fn insert_processed_message_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    mark: &ProcessedMessageMark,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO read_model_processed_messages (consumer_name, message_id)
        VALUES (?, ?)
        "#,
    )
    .bind(&mark.consumer_name)
    .bind(&mark.message_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn processed_message_key(mark: &ProcessedMessageMark) -> (String, String) {
    (mark.consumer_name.clone(), mark.message_id.clone())
}

async fn save_snapshot_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    identity: &StreamIdentity,
    record: SnapshotRecord,
) -> Result<(), RepositoryError> {
    validate_snapshot_identity(identity, &record)?;

    sqlx::query(
        r#"
        INSERT INTO aggregate_snapshots (
          aggregate_type,
          aggregate_id,
          version,
          snapshot_type,
          snapshot_version,
          payload,
          payload_codec,
          payload_codec_version,
          metadata,
          recorded_at
        )
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(aggregate_type, aggregate_id) DO UPDATE SET
          version = excluded.version,
          snapshot_type = excluded.snapshot_type,
          snapshot_version = excluded.snapshot_version,
          payload = excluded.payload,
          payload_codec = excluded.payload_codec,
          payload_codec_version = excluded.payload_codec_version,
          metadata = excluded.metadata,
          recorded_at = excluded.recorded_at,
          updated_at = CURRENT_TIMESTAMP
        "#,
    )
    .bind(identity.aggregate_type())
    .bind(identity.aggregate_id())
    .bind(sqlx_repository_i64_from_u64(
        SQLITE_BACKEND,
        record.version,
        "snapshot version",
        SIGNED_INTEGER_STORAGE,
    )?)
    .bind(&record.snapshot_type)
    .bind(sqlx_repository_i64_from_u64(
        SQLITE_BACKEND,
        record.snapshot_version,
        "snapshot payload version",
        SIGNED_INTEGER_STORAGE,
    )?)
    .bind(&record.payload)
    .bind(&record.payload_codec)
    .bind(i64::from(record.payload_codec_version))
    .bind(serialize_event_metadata(&record.metadata)?)
    .bind(system_time_to_storage(record.recorded_at)?)
    .execute(&mut **tx)
    .await
    .map_err(|err| repository_storage_error("save snapshot", err))?;

    Ok(())
}

fn snapshot_from_row(row: sqlx::sqlite::SqliteRow) -> Result<SnapshotRecord, RepositoryError> {
    let metadata_json: String = row
        .try_get("metadata")
        .map_err(|err| repository_storage_error("decode snapshot metadata row", err))?;
    Ok(SnapshotRecord {
        aggregate_type: row
            .try_get("aggregate_type")
            .map_err(|err| repository_storage_error("decode snapshot aggregate type row", err))?,
        aggregate_id: row
            .try_get("aggregate_id")
            .map_err(|err| repository_storage_error("decode snapshot aggregate id row", err))?,
        version: sqlx_repository_u64_from_i64(
            SQLITE_BACKEND,
            row.try_get("version")
                .map_err(|err| repository_storage_error("decode snapshot version row", err))?,
            "snapshot version",
        )?,
        snapshot_type: row
            .try_get("snapshot_type")
            .map_err(|err| repository_storage_error("decode snapshot type row", err))?,
        snapshot_version: sqlx_repository_u64_from_i64(
            SQLITE_BACKEND,
            row.try_get("snapshot_version").map_err(|err| {
                repository_storage_error("decode snapshot payload version row", err)
            })?,
            "snapshot payload version",
        )?,
        payload_codec: row
            .try_get("payload_codec")
            .map_err(|err| repository_storage_error("decode snapshot payload codec row", err))?,
        payload_codec_version: sqlx_repository_u16_from_i64(
            SQLITE_BACKEND,
            row.try_get("payload_codec_version").map_err(|err| {
                repository_storage_error("decode snapshot payload codec version row", err)
            })?,
            "snapshot payload codec version",
        )?,
        payload: row
            .try_get("payload")
            .map_err(|err| repository_storage_error("decode snapshot payload row", err))?,
        metadata: deserialize_event_metadata(&metadata_json)?,
        recorded_at: system_time_from_storage(
            row.try_get::<String, _>("recorded_at")
                .map_err(|err| repository_storage_error("decode snapshot recorded_at row", err))?
                .as_str(),
        ),
    })
}

fn next_document_version(
    collection: &str,
    id: &str,
    current_version: Option<u64>,
) -> Result<u64, ReadModelError> {
    match current_version {
        Some(version) => version.checked_add(1).ok_or_else(|| {
            ReadModelError::Storage(format!("read model version overflow for {collection}:{id}"))
        }),
        None => Ok(1),
    }
}

fn system_time_to_storage(timestamp: SystemTime) -> Result<String, RepositoryError> {
    let duration = timestamp.duration_since(UNIX_EPOCH).map_err(|err| {
        RepositoryError::Model(format!(
            "event timestamp before UNIX epoch cannot be stored in sqlite: {err}"
        ))
    })?;
    Ok(format!(
        "{}.{:09}",
        duration.as_secs(),
        duration.subsec_nanos()
    ))
}

fn system_time_to_epoch_secs(timestamp: SystemTime) -> Result<f64, RepositoryError> {
    let duration = timestamp.duration_since(UNIX_EPOCH).map_err(|err| {
        RepositoryError::Model(format!(
            "event timestamp before UNIX epoch cannot be compared in sqlite: {err}"
        ))
    })?;
    Ok(duration.as_secs_f64())
}

fn system_time_from_storage(value: &str) -> SystemTime {
    let Some((secs, nanos)) = value.split_once('.') else {
        return UNIX_EPOCH;
    };
    let Ok(secs) = secs.parse::<u64>() else {
        return UNIX_EPOCH;
    };
    let Ok(nanos) = nanos.parse::<u32>() else {
        return UNIX_EPOCH;
    };
    if nanos >= 1_000_000_000 {
        return UNIX_EPOCH;
    }
    UNIX_EPOCH + Duration::new(secs, nanos)
}

fn repository_storage_error(operation: &str, err: sqlx::Error) -> RepositoryError {
    sqlx_repo::repository_storage_error(SQLITE_BACKEND, operation, err)
}

fn read_model_storage_error(operation: &str, err: sqlx::Error) -> ReadModelError {
    sqlx_repo::read_model_storage_error(SQLITE_BACKEND, operation, err)
}

fn table_schema_storage_error(operation: &str, err: sqlx::Error) -> TableStoreError {
    TableStoreError::Storage(format!("{SQLITE_BACKEND} {operation} failed: {err}"))
}
