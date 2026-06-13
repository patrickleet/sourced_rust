//! SQLite-backed async repository and transactional relational read-model writes.
//!
//! This adapter is a local SQL persistence backend for the async repository
//! boundary. It is feature-gated behind `sqlite` and is intentionally async-only.

#![expect(
    clippy::manual_async_fn,
    reason = "async trait impls return impl Future + Send to preserve public Send bounds"
)]

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sqlx::sqlite::{SqlitePoolOptions, SqliteRow};
use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool, Transaction};

use crate::entity::{Entity, EventRecord};
use crate::outbox::{OutboxMessage, OutboxMessageStatus};
use crate::outbox_worker::{
    ensure_active_claim, AsyncOutboxStore, ClaimOutboxMessages, OutboxClaimRef,
};
use crate::read_model::{
    ColumnDef, ColumnType, ReadModelAdapterCapabilities, ReadModelCommitOutcome, ReadModelError,
    ReadModelLoadGraph, ReadModelLoadRequest, ReadModelQueryCapabilities, ReadModelWritePlan,
    RowValue,
};
use crate::repository::{
    reject_duplicate_outbox_messages, reject_duplicate_streams,
    validate_entity_id_matches_identity, validate_prepared_appends, validate_snapshot_identity,
    validate_supported_event_codec, CommitBatch, GetStream, InboxReceipt, InboxStore,
    PreparedEventAppend, ReadModelWritePlanStore, RelationalReadModelQueryStore, RepositoryError,
    SnapshotStore, SnapshotWrite, StreamIdentity, TransactionalCommit,
};
use crate::snapshot::SnapshotRecord;
use crate::sqlx_repo::read_model::{
    apply_read_model_write_plan_in_tx, commit_read_model_write_plan, empty_string_as_none,
    load_read_model_graph, quote_identifier, remember_read_model_schemas,
    sql_read_model_capabilities, validate_sql_write_plan,
};
use crate::sqlx_repo::{
    self, audited_table_schema_sql, deserialize_event_metadata, is_sqlite_unique_constraint,
    read_model_i64_from_u64 as sqlx_read_model_i64_from_u64,
    read_model_u64_from_i64 as sqlx_read_model_u64_from_i64,
    repository_i64_from_u64 as sqlx_repository_i64_from_u64,
    repository_u16_from_i64 as sqlx_repository_u16_from_i64,
    repository_u64_from_i64 as sqlx_repository_u64_from_i64, serialize_event_metadata,
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
    read_model_schemas: Arc<RwLock<TableSchemaRegistry>>,
}

/// SQLite-backed outbox table store.
#[derive(Clone)]
pub struct SqliteOutboxStore {
    pool: SqlitePool,
}

impl SqliteRepository {
    /// Create a repository from an existing migrated pool.
    pub fn new(pool: SqlitePool) -> Self {
        Self {
            pool,
            read_model_schemas: Arc::new(RwLock::new(TableSchemaRegistry::new())),
        }
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
            sqlx::query(audited_table_schema_sql(statement))
                .execute(&self.pool)
                .await
                .map_err(|err| table_schema_storage_error("bootstrap table schema", err))?;
        }
        remember_read_model_schemas(&self.read_model_schemas, registry)?;
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
            sqlx::query(audited_table_schema_sql(statement))
                .execute(&self.pool)
                .await
                .map_err(|err| table_schema_storage_error("bootstrap table schema", err))?;
        }
        Ok(table_schema_bootstrap_result(registry))
    }
}

impl GetStream for SqliteRepository {
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
            if identities.is_empty() {
                return Ok(Vec::new());
            }

            // Group ids by aggregate type so each type is one `aggregate_id IN
            // (...)` round trip instead of a query per identity. SQLite has no
            // array type, so the id list is built as bound placeholders.
            // `get_all` builds single-type batches, so the common case is one
            // query.
            let mut ids_by_type: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
            for identity in identities {
                ids_by_type
                    .entry(identity.aggregate_type())
                    .or_default()
                    .push(identity.aggregate_id());
            }

            let mut entities = Vec::with_capacity(identities.len());
            for (aggregate_type, aggregate_ids) in ids_by_type {
                // Ordering by aggregate_id then sequence lets us slice the flat
                // result into per-aggregate entities in one pass. Callers of
                // `get_all` accept storage-order results.
                let mut builder = QueryBuilder::<Sqlite>::new(
                    "SELECT aggregate_id, event_name, event_version, payload, \
                     payload_codec, payload_codec_version, metadata, sequence, recorded_at \
                     FROM aggregate_events WHERE aggregate_type = ",
                );
                builder.push_bind(aggregate_type);
                builder.push(" AND aggregate_id IN (");
                let mut separated = builder.separated(", ");
                for id in &aggregate_ids {
                    separated.push_bind(*id);
                }
                builder.push(") ORDER BY aggregate_id ASC, sequence ASC");

                let rows = builder
                    .build()
                    .fetch_all(&self.pool)
                    .await
                    .map_err(|err| repository_storage_error("load streams", err))?;

                let mut current_id: Option<String> = None;
                let mut current_events: Vec<EventRecord> = Vec::new();
                for row in rows {
                    let row_id: String = row
                        .try_get("aggregate_id")
                        .map_err(|err| repository_storage_error("decode aggregate id row", err))?;
                    let event = event_from_row(row)?;
                    match &current_id {
                        Some(id) if id == &row_id => current_events.push(event),
                        _ => {
                            if let Some(id) = current_id.take() {
                                entities.push(entity_from_events(
                                    id,
                                    std::mem::take(&mut current_events),
                                ));
                            }
                            current_id = Some(row_id);
                            current_events.push(event);
                        }
                    }
                }
                if let Some(id) = current_id.take() {
                    entities.push(entity_from_events(id, current_events));
                }
            }

            Ok(entities)
        }
    }

    fn get_stream_tail<'a>(
        &'a self,
        identity: &'a StreamIdentity,
        after_version: u64,
    ) -> impl Future<Output = Result<Option<Entity>, RepositoryError>> + Send + 'a {
        async move {
            // Fetch only the post-snapshot tail. `after_version` is the snapshot
            // version (an event sequence); `sequence > ?` skips already-folded
            // rows so a fresh snapshot over a long stream no longer reads and
            // decodes the entire history.
            let after = sqlx_repository_i64_from_u64(
                SQLITE_BACKEND,
                after_version,
                "snapshot tail lower bound",
                SIGNED_INTEGER_STORAGE,
            )?;
            let rows = sqlx::query(
                r#"
                SELECT event_name, event_version, payload, payload_codec,
                       payload_codec_version, metadata, sequence, recorded_at
                FROM aggregate_events
                WHERE aggregate_type = ? AND aggregate_id = ? AND sequence > ?
                ORDER BY sequence ASC
                "#,
            )
            .bind(identity.aggregate_type())
            .bind(identity.aggregate_id())
            .bind(after)
            .fetch_all(&self.pool)
            .await
            .map_err(|err| repository_storage_error("load stream tail", err))?;

            // An empty tail is ambiguous from this query alone (no rows could
            // mean "snapshot is current" or "stream does not exist"). The
            // snapshot hydrate path only calls this after confirming a snapshot
            // exists for the identity, so an empty tail means the snapshot is
            // current. Return an entity at exactly `after_version`.
            let mut events = Vec::with_capacity(rows.len());
            for row in rows {
                events.push(event_from_row(row)?);
            }

            let mut entity = Entity::new();
            entity.set_id(identity.aggregate_id());
            entity.load_tail_from_history(events, after_version);
            Ok(Some(entity))
        }
    }
}

impl TransactionalCommit for SqliteRepository {
    fn commit_batch<'a>(
        &'a self,
        batch: CommitBatch<'a>,
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
                validate_sql_write_plan(plan)?;
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

            insert_events_in_tx(&mut tx, &prepared).await?;

            insert_outbox_messages_in_tx(&mut tx, &batch.outbox_messages).await?;

            for plan in batch.read_model_plans {
                apply_read_model_write_plan_in_tx(&mut tx, plan).await?;
            }

            for write in batch.snapshots {
                match write {
                    SnapshotWrite::Save { identity, record } => {
                        save_snapshot_in_tx(&mut tx, &identity, record).await?;
                    }
                }
            }

            for receipt in &batch.inbox_receipts {
                insert_inbox_receipt_in_tx(&mut tx, receipt).await?;
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

impl InboxStore for SqliteRepository {
    fn inbox_contains<'a>(
        &'a self,
        consumer: &'a str,
        message_id: &'a str,
    ) -> impl Future<Output = Result<bool, RepositoryError>> + Send + 'a {
        async move {
            let row = sqlx::query(
                "SELECT 1 FROM consumer_inbox WHERE consumer = ? AND message_id = ? LIMIT 1",
            )
            .bind(consumer)
            .bind(message_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|err| repository_storage_error("query consumer inbox", err))?;
            Ok(row.is_some())
        }
    }

    fn purge_inbox_older_than(
        &self,
        age: std::time::Duration,
    ) -> impl Future<Output = Result<u64, RepositoryError>> + Send {
        async move {
            // `processed_at` defaults to CURRENT_TIMESTAMP (UTC `YYYY-MM-DD
            // HH:MM:SS`), so compare against the database clock via
            // `datetime('now', '-N seconds')` — no client/server skew.
            let modifier = format!("-{} seconds", age.as_secs());
            let result =
                sqlx::query("DELETE FROM consumer_inbox WHERE processed_at < datetime('now', ?)")
                    .bind(modifier)
                    .execute(&self.pool)
                    .await
                    .map_err(|err| repository_storage_error("purge consumer inbox", err))?;
            Ok(result.rows_affected())
        }
    }
}

impl ReadModelWritePlanStore for SqliteRepository {
    fn read_model_capabilities(&self) -> ReadModelAdapterCapabilities {
        sql_read_model_capabilities()
    }

    fn commit_write_plan(
        &self,
        plan: ReadModelWritePlan,
    ) -> impl Future<Output = Result<ReadModelCommitOutcome, ReadModelError>> + Send + '_ {
        async move { commit_read_model_write_plan(&self.pool, plan).await }
    }
}

impl RelationalReadModelQueryStore for SqliteRepository {
    fn read_model_query_capabilities(&self) -> ReadModelQueryCapabilities {
        ReadModelQueryCapabilities::relationship_includes()
    }

    fn load_graph(
        &self,
        request: ReadModelLoadRequest,
    ) -> impl Future<Output = Result<ReadModelLoadGraph, ReadModelError>> + Send + '_ {
        async move {
            load_read_model_graph(
                &self.pool,
                &self.read_model_schemas,
                request,
                self.read_model_query_capabilities(),
            )
            .await
        }
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

            // Explicit ids (after-commit immediate dispatch) bypass the ordered
            // candidate scan; the per-id conditional UPDATE below still enforces
            // claimability and destination, so raced/unclaimable ids are skipped.
            let candidate_ids: Vec<String> = if let Some(ids) = request.message_ids.clone() {
                ids
            } else {
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
                .map_err(|err| {
                    repository_storage_error("select claimable outbox messages", err)
                })?;
                let mut ids = Vec::with_capacity(candidate_rows.len());
                for row in candidate_rows {
                    ids.push(row.try_get::<String, _>("message_id").map_err(|err| {
                        repository_storage_error("decode outbox message id row", err)
                    })?);
                }
                ids
            };

            let mut claimed = Vec::new();
            for message_id in candidate_ids {
                if claimed.len() >= request.batch_size {
                    break;
                }
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
}

impl SnapshotStore for SqliteRepository {
    fn get_snapshot<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<Option<SnapshotRecord>, RepositoryError>> + Send + 'a {
        async move {
            let row = sqlx::query(
                r#"
                SELECT aggregate_type, aggregate_id, version,
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

    fn save_snapshot<'a>(
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

    fn delete_snapshot<'a>(
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

/// Record a consumer inbox receipt in the commit transaction. The
/// `(consumer, message_id)` primary key is the dedupe gate: a unique violation
/// means the message was already processed, so the whole batch rolls back and the
/// effects are not double-applied. `processed_at` defaults to `CURRENT_TIMESTAMP`.
async fn insert_inbox_receipt_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    receipt: &InboxReceipt,
) -> Result<(), RepositoryError> {
    receipt.validate()?;
    let result = sqlx::query("INSERT INTO consumer_inbox (consumer, message_id) VALUES (?, ?)")
        .bind(&receipt.consumer)
        .bind(&receipt.message_id)
        .execute(&mut **tx)
        .await;
    match result {
        Ok(_) => Ok(()),
        Err(err) if is_sqlite_unique_constraint(&err) => {
            Err(RepositoryError::DuplicateInboxReceipt {
                consumer: receipt.consumer.clone(),
                message_id: receipt.message_id.clone(),
            })
        }
        Err(err) => Err(repository_storage_error(
            "insert consumer inbox receipt",
            err,
        )),
    }
}

/// Insert every outbox message with multi-row INSERTs (chunked to respect
/// SQLite's bound-parameter limit). A unique constraint violation on
/// `message_id` still maps to `DuplicateOutboxMessageInBatch`.
async fn insert_outbox_messages_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    messages: &[OutboxMessage],
) -> Result<(), RepositoryError> {
    struct OutboxRow<'a> {
        message_id: &'a str,
        event_type: &'a str,
        payload: &'a [u8],
        payload_codec: &'a str,
        payload_codec_version: i64,
        destination: Option<&'a str>,
        metadata: String,
        status: &'a str,
        created_at: String,
        worker_id: Option<&'a str>,
        leased_until: Option<String>,
        attempts: i64,
        last_error: Option<&'a str>,
        source_aggregate_type: Option<&'a str>,
        source_aggregate_id: Option<&'a str>,
        source_sequence: Option<i64>,
        correlation_id: Option<&'a str>,
        causation_id: Option<&'a str>,
    }

    let mut rows = Vec::with_capacity(messages.len());
    for message in messages {
        rows.push(OutboxRow {
            message_id: message.id(),
            event_type: &message.event_type,
            payload: &message.payload,
            payload_codec: &message.payload_codec,
            payload_codec_version: i64::from(message.payload_codec_version),
            destination: message.destination.as_deref(),
            metadata: serialize_event_metadata(&message.metadata)?,
            status: message.status.as_str(),
            created_at: system_time_to_storage(message.created_at)?,
            worker_id: message.worker_id.as_deref(),
            leased_until: message
                .leased_until
                .map(system_time_to_storage)
                .transpose()?,
            attempts: i64::from(message.attempts),
            last_error: message.last_error.as_deref(),
            source_aggregate_type: message.source_aggregate_type.as_deref(),
            source_aggregate_id: message.source_aggregate_id.as_deref(),
            source_sequence: message
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
            correlation_id: message.correlation_id(),
            causation_id: message.causation_id(),
        });
    }

    for chunk in rows.chunks(SQLITE_MAX_BIND_PARAMS / OUTBOX_BIND_COLUMNS) {
        let mut builder = QueryBuilder::<Sqlite>::new(
            "INSERT INTO outbox_messages (\
             message_id, event_type, payload, payload_codec, payload_codec_version, \
             destination, metadata, status, created_at, next_available_at, \
             claimed_by, claimed_until, attempts, last_error, source_aggregate_type, \
             source_aggregate_id, source_sequence, correlation_id, causation_id) ",
        );
        builder.push_values(chunk, |mut row, message| {
            row.push_bind(message.message_id)
                .push_bind(message.event_type)
                .push_bind(message.payload)
                .push_bind(message.payload_codec)
                .push_bind(message.payload_codec_version)
                .push_bind(message.destination)
                .push_bind(message.metadata.as_str())
                .push_bind(message.status)
                .push_bind(message.created_at.as_str())
                // created_at and next_available_at share the same value.
                .push_bind(message.created_at.as_str())
                .push_bind(message.worker_id)
                .push_bind(message.leased_until.as_deref())
                .push_bind(message.attempts)
                .push_bind(message.last_error)
                .push_bind(message.source_aggregate_type)
                .push_bind(message.source_aggregate_id)
                .push_bind(message.source_sequence)
                .push_bind(message.correlation_id)
                .push_bind(message.causation_id);
        });

        let result = builder.build().execute(&mut **tx).await;
        if let Err(err) = result {
            if is_sqlite_unique_constraint(&err) {
                // The batch was already deduped, so a violation means the id
                // collides with a previously committed row. Report the first id
                // in the chunk, matching the per-row path's contract.
                return Err(RepositoryError::DuplicateOutboxMessageInBatch {
                    id: chunk[0].message_id.to_string(),
                });
            }
            return Err(repository_storage_error("insert outbox messages", err));
        }
    }

    Ok(())
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

/// Maximum bound parameters per statement. SQLite's historical limit is 999, so
/// staying under it keeps the batched inserts portable across SQLite builds.
const SQLITE_MAX_BIND_PARAMS: usize = 900;

/// Bound parameters per `aggregate_events` row.
const EVENT_BIND_COLUMNS: usize = 10;

/// Bound parameters per `outbox_messages` row.
const OUTBOX_BIND_COLUMNS: usize = 19;

/// Insert every event across all prepared appends with multi-row INSERTs
/// (chunked to respect SQLite's bound-parameter limit).
///
/// Conflict detection is unchanged from the per-row path: the `(aggregate_type,
/// aggregate_id, sequence)` primary key is the contiguity gate, and a unique
/// constraint violation still surfaces as `ConcurrentWrite`. SQLite does not
/// abort the transaction on a constraint error, so the conflicting stream's
/// actual version is re-read in the same transaction, exactly as before.
async fn insert_events_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    prepared: &[PreparedEventAppend],
) -> Result<(), RepositoryError> {
    struct EventRow<'a> {
        identity: &'a StreamIdentity,
        expected_version: u64,
        sequence: i64,
        event_name: &'a str,
        event_version: i64,
        payload: &'a [u8],
        payload_codec: &'a str,
        payload_codec_version: i64,
        metadata: String,
        recorded_at: String,
    }

    let mut rows = Vec::new();
    for append in prepared {
        for event in &append.events {
            rows.push(EventRow {
                identity: &append.identity,
                expected_version: append.expected_version,
                sequence: sqlx_repository_i64_from_u64(
                    SQLITE_BACKEND,
                    event.sequence,
                    "sequence",
                    SIGNED_INTEGER_STORAGE,
                )?,
                event_name: &event.event_name,
                event_version: sqlx_repository_i64_from_u64(
                    SQLITE_BACKEND,
                    event.event_version,
                    "event_version",
                    SIGNED_INTEGER_STORAGE,
                )?,
                payload: &event.payload,
                payload_codec: &event.payload_codec,
                payload_codec_version: i64::from(event.payload_codec_version),
                metadata: serialize_event_metadata(&event.metadata)?,
                recorded_at: system_time_to_storage(event.timestamp)?,
            });
        }
    }

    for chunk in rows.chunks(SQLITE_MAX_BIND_PARAMS / EVENT_BIND_COLUMNS) {
        let mut builder = QueryBuilder::<Sqlite>::new(
            "INSERT INTO aggregate_events (\
             aggregate_type, aggregate_id, sequence, event_name, event_version, \
             payload, payload_codec, payload_codec_version, metadata, recorded_at) ",
        );
        builder.push_values(chunk, |mut row, event| {
            row.push_bind(event.identity.aggregate_type())
                .push_bind(event.identity.aggregate_id())
                .push_bind(event.sequence)
                .push_bind(event.event_name)
                .push_bind(event.event_version)
                .push_bind(event.payload)
                .push_bind(event.payload_codec)
                .push_bind(event.payload_codec_version)
                .push_bind(event.metadata.as_str())
                .push_bind(event.recorded_at.as_str());
        });

        let result = builder.build().execute(&mut **tx).await;
        if let Err(err) = result {
            if is_sqlite_unique_constraint(&err) {
                // Find the conflicting stream (its actual version no longer
                // matches its expected version) and report it.
                for event in chunk {
                    let actual = stream_version_in_tx(tx, event.identity).await?;
                    if actual != event.expected_version {
                        return Err(RepositoryError::ConcurrentWrite {
                            id: event.identity.to_string(),
                            expected: event.expected_version,
                            actual,
                        });
                    }
                }
                let event = &chunk[0];
                let actual = stream_version_in_tx(tx, event.identity).await?;
                return Err(RepositoryError::ConcurrentWrite {
                    id: event.identity.to_string(),
                    expected: event.expected_version,
                    actual,
                });
            }
            return Err(repository_storage_error("insert events", err));
        }
    }

    Ok(())
}

fn entity_from_events(aggregate_id: String, events: Vec<EventRecord>) -> Entity {
    let mut entity = Entity::new();
    entity.set_id(aggregate_id);
    entity.load_from_history(events);
    entity
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

impl crate::sqlx_repo::read_model::SqlxReadModelBackend for Sqlite {
    const BACKEND: &'static str = SQLITE_BACKEND;
    const INTEGER_STORAGE: &'static str = SIGNED_INTEGER_STORAGE;

    fn push_row_value_bind(
        builder: &mut QueryBuilder<Sqlite>,
        value: RowValue,
        column: &ColumnDef,
    ) -> Result<(), ReadModelError> {
        match value {
            RowValue::Null => Self::push_null_bind(builder, column)?,
            RowValue::Bool(value) => {
                builder.push_bind(i64::from(value));
            }
            RowValue::I64(value) => {
                builder.push_bind(value);
            }
            RowValue::U64(value) => {
                builder.push_bind(sqlx_read_model_i64_from_u64(
                    SQLITE_BACKEND,
                    value,
                    &column.column_name,
                    SIGNED_INTEGER_STORAGE,
                )?);
            }
            RowValue::F64(value) => {
                builder.push_bind(value);
            }
            RowValue::String(value) => {
                builder.push_bind(value);
            }
            RowValue::Bytes(value) => {
                builder.push_bind(value);
            }
            RowValue::Json(value) => {
                let payload = serde_json::to_string(&value)
                    .map_err(|err| ReadModelError::Serde(err.to_string()))?;
                builder.push_bind(payload);
            }
        }
        Ok(())
    }

    fn push_null_bind(
        builder: &mut QueryBuilder<Sqlite>,
        column: &ColumnDef,
    ) -> Result<(), ReadModelError> {
        match &column.column_type {
            ColumnType::Text | ColumnType::Json | ColumnType::Timestamp => {
                builder.push_bind(Option::<String>::None);
            }
            ColumnType::Boolean | ColumnType::Integer | ColumnType::UnsignedInteger => {
                builder.push_bind(Option::<i64>::None);
            }
            ColumnType::Float => {
                builder.push_bind(Option::<f64>::None);
            }
            ColumnType::Bytes => {
                builder.push_bind(Option::<Vec<u8>>::None);
            }
            ColumnType::Unsupported(type_name) => {
                return Err(ReadModelError::Metadata(format!(
                    "read model column `{}` has unsupported type `{}`",
                    column.column_name, type_name
                )));
            }
        }
        Ok(())
    }

    fn rows_affected(result: &sqlx::sqlite::SqliteQueryResult) -> u64 {
        result.rows_affected()
    }

    fn push_select_column(builder: &mut QueryBuilder<Sqlite>, column: &ColumnDef) {
        builder.push(quote_identifier(&column.column_name));
    }

    fn row_value(row: &SqliteRow, column: &ColumnDef) -> Result<RowValue, ReadModelError> {
        Ok(match column.column_type {
            ColumnType::Text | ColumnType::Timestamp => row
                .try_get::<Option<String>, _>(column.column_name.as_str())
                .map_err(|err| read_model_storage_error("decode relational text column", err))?
                .map(RowValue::String)
                .unwrap_or(RowValue::Null),
            ColumnType::Boolean => row
                .try_get::<Option<i64>, _>(column.column_name.as_str())
                .map_err(|err| read_model_storage_error("decode relational boolean column", err))?
                .map(|value| RowValue::Bool(value != 0))
                .unwrap_or(RowValue::Null),
            ColumnType::Integer => row
                .try_get::<Option<i64>, _>(column.column_name.as_str())
                .map_err(|err| read_model_storage_error("decode relational integer column", err))?
                .map(RowValue::I64)
                .unwrap_or(RowValue::Null),
            ColumnType::UnsignedInteger => row
                .try_get::<Option<i64>, _>(column.column_name.as_str())
                .map_err(|err| {
                    read_model_storage_error("decode relational unsigned integer column", err)
                })?
                .map(|value| {
                    sqlx_read_model_u64_from_i64(SQLITE_BACKEND, value, column.column_name.as_str())
                        .map(RowValue::U64)
                })
                .transpose()?
                .unwrap_or(RowValue::Null),
            ColumnType::Float => row
                .try_get::<Option<f64>, _>(column.column_name.as_str())
                .map_err(|err| read_model_storage_error("decode relational float column", err))?
                .map(RowValue::F64)
                .unwrap_or(RowValue::Null),
            ColumnType::Bytes => row
                .try_get::<Option<Vec<u8>>, _>(column.column_name.as_str())
                .map_err(|err| read_model_storage_error("decode relational bytes column", err))?
                .map(RowValue::Bytes)
                .unwrap_or(RowValue::Null),
            ColumnType::Json => row
                .try_get::<Option<String>, _>(column.column_name.as_str())
                .map_err(|err| read_model_storage_error("decode relational json column", err))?
                .map(|payload| {
                    serde_json::from_str(&payload)
                        .map(RowValue::Json)
                        .map_err(|err| ReadModelError::Serde(err.to_string()))
                })
                .transpose()?
                .unwrap_or(RowValue::Null),
            ColumnType::Unsupported(ref type_name) => {
                return Err(ReadModelError::Metadata(format!(
                    "read model `{}` column `{}` has unsupported type `{}`",
                    column.field_name, column.column_name, type_name
                )));
            }
        })
    }
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
          snapshot_version,
          payload,
          payload_codec,
          payload_codec_version,
          metadata,
          recorded_at
        )
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(aggregate_type, aggregate_id) DO UPDATE SET
          version = excluded.version,
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
