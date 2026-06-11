//! Postgres-backed async repository and transactional relational read-model writes.
//!
//! This adapter is the production-oriented SQL event-store path. It is
//! feature-gated behind `postgres` and async-only.

#![expect(
    clippy::manual_async_fn,
    reason = "async trait impls return impl Future + Send to preserve public Send bounds"
)]

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sqlx::postgres::{PgPoolOptions, PgRow};
use sqlx::{PgPool, Postgres, QueryBuilder, Row, Transaction};

use crate::entity::Entity;
use crate::entity::EventRecord;
use crate::outbox::{OutboxMessage, OutboxMessageStatus};
use crate::outbox_worker::{
    ensure_active_claim, AsyncOutboxStore, ClaimOutboxMessages, OutboxClaimRef,
    OutboxPublishFailureAction,
};
use crate::read_model::{
    column_name_for, key_fingerprint, validate_key, validate_row_values, ColumnDef, ColumnType,
    DeleteRowMutation, ExpectedVersion, PatchMode, PatchRowMutation, ReadModelAdapterCapabilities,
    ReadModelCommitOutcome, ReadModelError, ReadModelIncludeRows, ReadModelLoadGraph,
    ReadModelLoadRequest, ReadModelMutation, ReadModelQueryCapabilities, ReadModelSchema,
    ReadModelWritePlan, RelationshipDef, RelationshipKind, RowKey, RowMutation, RowValue,
    RowValues, RowWriteMode, Versioned,
};
use crate::repository::{
    CommitBatch, GetStream, InboxReceipt, InboxStore, PreparedEventAppend, ReadModelWritePlanStore,
    RelationalReadModelQueryStore, RepositoryError, SnapshotStore, SnapshotWrite, StreamIdentity,
    TransactionalCommit,
};
use crate::snapshot::SnapshotRecord;
use crate::sqlx_repo::{
    self, audited_table_schema_sql, deserialize_event_metadata, is_postgres_unique_violation,
    read_model_i64_from_u64 as sqlx_read_model_i64_from_u64,
    read_model_u64_from_i64 as sqlx_read_model_u64_from_i64, reject_duplicate_outbox_messages,
    reject_duplicate_streams, repository_i32_from_u64 as sqlx_repository_i32_from_u64,
    repository_i64_from_u64 as sqlx_repository_i64_from_u64,
    repository_u16_from_i32 as sqlx_repository_u16_from_i32,
    repository_u64_from_i32 as sqlx_repository_u64_from_i32,
    repository_u64_from_i64 as sqlx_repository_u64_from_i64, serialize_event_metadata,
    validate_entity_id_matches_identity, validate_prepared_appends, validate_snapshot_identity,
    validate_supported_event_codec,
};
use crate::table::{
    generate_table_migration_artifacts, table_schema_bootstrap_result, table_schema_statements,
    TableMigrationArtifact, TableSchemaBootstrap, TableSchemaRegistry, TableSqlDialect,
    TableSqlSchemaAdapter, TableStoreError,
};

const POSTGRES_SCHEMA: &str = include_str!("../../migrations/postgres/0001_initial.sql");
const POSTGRES_BACKEND: &str = "postgres";
const BIGINT_STORAGE: &str = "bigint storage";
const INTEGER_STORAGE: &str = "integer storage";

/// Postgres-backed async repository.
#[derive(Clone)]
pub struct PostgresRepository {
    pool: PgPool,
    read_model_schemas: Arc<RwLock<TableSchemaRegistry>>,
}

/// Postgres-backed outbox table store.
#[derive(Clone)]
pub struct PostgresOutboxStore {
    pool: PgPool,
}

impl PostgresRepository {
    /// Create a repository from an existing migrated pool.
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            read_model_schemas: Arc::new(RwLock::new(TableSchemaRegistry::new())),
        }
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

    /// SQL artifact adapter for registered table/read-model schemas.
    pub fn table_schema_adapter(&self) -> TableSqlSchemaAdapter {
        TableSqlSchemaAdapter::postgres()
    }

    /// Generate SQL statements for registered table/read-model schemas.
    pub fn generate_table_migration_artifacts(
        &self,
        registry: &TableSchemaRegistry,
    ) -> Result<Vec<TableMigrationArtifact>, TableStoreError> {
        generate_table_migration_artifacts(registry, TableSqlDialect::Postgres)
    }

    /// Explicit dev/test bootstrap for registered table/read-model schemas.
    pub async fn bootstrap_table_schema_for_dev(
        &self,
        registry: &TableSchemaRegistry,
    ) -> Result<TableSchemaBootstrap, TableStoreError> {
        for statement in table_schema_statements(registry, TableSqlDialect::Postgres)? {
            sqlx::query(audited_table_schema_sql(statement))
                .execute(&self.pool)
                .await
                .map_err(|err| table_schema_storage_error("bootstrap table schema", err))?;
        }
        remember_read_model_schemas(&self.read_model_schemas, registry)?;
        Ok(table_schema_bootstrap_result(registry))
    }

    /// Access an outbox-store handle backed by this repository's pool.
    pub fn outbox_store(&self) -> PostgresOutboxStore {
        PostgresOutboxStore {
            pool: self.pool.clone(),
        }
    }
}

impl PostgresOutboxStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// SQL artifact adapter for registered table/read-model schemas.
    pub fn table_schema_adapter(&self) -> TableSqlSchemaAdapter {
        TableSqlSchemaAdapter::postgres()
    }

    /// Generate SQL statements for registered table/read-model schemas.
    pub fn generate_table_migration_artifacts(
        &self,
        registry: &TableSchemaRegistry,
    ) -> Result<Vec<TableMigrationArtifact>, TableStoreError> {
        generate_table_migration_artifacts(registry, TableSqlDialect::Postgres)
    }

    /// Explicit dev/test bootstrap for registered table/read-model schemas.
    pub async fn bootstrap_table_schema_for_dev(
        &self,
        registry: &TableSchemaRegistry,
    ) -> Result<TableSchemaBootstrap, TableStoreError> {
        for statement in table_schema_statements(registry, TableSqlDialect::Postgres)? {
            sqlx::query(audited_table_schema_sql(statement))
                .execute(&self.pool)
                .await
                .map_err(|err| table_schema_storage_error("bootstrap table schema", err))?;
        }
        Ok(table_schema_bootstrap_result(registry))
    }
}

impl GetStream for PostgresRepository {
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
            if identities.is_empty() {
                return Ok(Vec::new());
            }

            // Group ids by aggregate type so each type is one `aggregate_id =
            // ANY($2)` round trip instead of a query per identity. `get_all`
            // builds single-type batches, so the common case is one query; the
            // grouping only exists to keep arbitrary mixed-type inputs correct.
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
                let rows = sqlx::query(
                    r#"
                    SELECT aggregate_id,
                           event_name,
                           event_version,
                           payload,
                           payload_codec,
                           payload_codec_version,
                           metadata::text AS metadata,
                           sequence,
                           EXTRACT(EPOCH FROM recorded_at)::double precision AS recorded_at_epoch
                    FROM aggregate_events
                    WHERE aggregate_type = $1 AND aggregate_id = ANY($2)
                    ORDER BY aggregate_id ASC, sequence ASC
                    "#,
                )
                .bind(aggregate_type)
                .bind(&aggregate_ids)
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
            // version (an event sequence); `sequence > $3` skips already-folded
            // rows so a fresh snapshot over a long stream no longer reads and
            // decodes the entire history.
            let after = sqlx_repository_i64_from_u64(
                POSTGRES_BACKEND,
                after_version,
                "snapshot tail lower bound",
                BIGINT_STORAGE,
            )?;
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
                WHERE aggregate_type = $1 AND aggregate_id = $2 AND sequence > $3
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

impl TransactionalCommit for PostgresRepository {
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

            insert_events_in_tx(&self.pool, &mut tx, &prepared).await?;

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

impl InboxStore for PostgresRepository {
    fn inbox_contains<'a>(
        &'a self,
        consumer: &'a str,
        message_id: &'a str,
    ) -> impl Future<Output = Result<bool, RepositoryError>> + Send + 'a {
        async move {
            let row = sqlx::query(
                "SELECT 1 FROM consumer_inbox WHERE consumer = $1 AND message_id = $2 LIMIT 1",
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
            // Compare against the database clock (`now()`) to avoid client/server
            // skew; `make_interval` takes the cutoff age in whole seconds.
            let secs = age.as_secs() as f64;
            let result = sqlx::query(
                "DELETE FROM consumer_inbox \
                 WHERE processed_at < now() - make_interval(secs => $1)",
            )
            .bind(secs)
            .execute(&self.pool)
            .await
            .map_err(|err| repository_storage_error("purge consumer inbox", err))?;
            Ok(result.rows_affected())
        }
    }
}

impl ReadModelWritePlanStore for PostgresRepository {
    fn read_model_capabilities(&self) -> ReadModelAdapterCapabilities {
        sql_read_model_capabilities()
    }

    fn commit_write_plan(
        &self,
        plan: ReadModelWritePlan,
    ) -> impl Future<Output = Result<ReadModelCommitOutcome, ReadModelError>> + Send + '_ {
        async move {
            validate_sql_write_plan(&plan)?;
            let mut tx = begin_read_model_tx(&self.pool).await?;
            let outcome = apply_read_model_write_plan_in_tx(&mut tx, plan).await?;
            commit_read_model_tx(tx).await?;
            Ok(outcome)
        }
    }
}

impl RelationalReadModelQueryStore for PostgresRepository {
    fn read_model_query_capabilities(&self) -> ReadModelQueryCapabilities {
        ReadModelQueryCapabilities::relationship_includes()
    }

    fn load_graph(
        &self,
        request: ReadModelLoadRequest,
    ) -> impl Future<Output = Result<ReadModelLoadGraph, ReadModelError>> + Send + '_ {
        async move {
            request.validate_for_query_capabilities(&self.read_model_query_capabilities())?;

            let (root_schema, include_specs) =
                resolve_registered_read_model_schemas(&self.read_model_schemas, &request)?;
            validate_key(&root_schema, &request.key)?;

            let Some(root) =
                load_postgres_relational_row_by_key(&self.pool, &root_schema, &request.key).await?
            else {
                return Ok(ReadModelLoadGraph::default());
            };

            let mut includes = BTreeMap::new();
            for spec in include_specs {
                let loaded_rows =
                    load_postgres_relationship_rows(&self.pool, &root_schema, &root.data, &spec)
                        .await?;
                includes.insert(
                    spec.name,
                    ReadModelIncludeRows {
                        relationship: spec.relationship,
                        target_schema: spec.target_schema,
                        rows: loaded_rows,
                    },
                );
            }

            Ok(ReadModelLoadGraph {
                root: Some(root),
                includes,
            })
        }
    }
}

impl AsyncOutboxStore for PostgresOutboxStore {
    fn messages_by_status_async(
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
            let claimed_until_epoch = system_time_to_epoch_secs(claimed_until)?;
            let limit = sqlx_repository_i64_from_u64(
                POSTGRES_BACKEND,
                request.batch_size as u64,
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
                  WHERE (
                    (status = $1 AND next_available_at <= to_timestamp($2))
                    OR (status = $3 AND (claimed_until IS NULL OR claimed_until <= to_timestamp($2)))
                  )
                    AND ($4::text IS NULL OR destination = $4)
                    AND ($9::text[] IS NULL OR message_id = ANY($9::text[]))
                  ORDER BY created_at ASC, message_id ASC
                  LIMIT $5
                  FOR UPDATE SKIP LOCKED
                )
                UPDATE outbox_messages AS message
                SET status = $6,
                    claimed_by = $7,
                    claimed_until = to_timestamp($8),
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
            .bind(request.destination.as_deref())
            .bind(limit)
            .bind(OutboxMessageStatus::InFlight.as_str())
            .bind(&request.worker_id)
            .bind(claimed_until_epoch)
            .bind(request.message_ids.as_deref())
            .fetch_all(&mut *tx)
            .await
            .map_err(|err| repository_storage_error("claim outbox messages", err))?;

            tx.commit()
                .await
                .map_err(|err| repository_storage_error("commit outbox claim transaction", err))?;

            rows.into_iter().map(outbox_message_from_row).collect()
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
                  AND attempts = $7
                "#,
            )
            .bind(OutboxMessageStatus::Published.as_str())
            .bind(now_epoch)
            .bind(&claim.message_id)
            .bind(OutboxMessageStatus::InFlight.as_str())
            .bind(&claim.worker_id)
            .bind(now_epoch)
            .bind(sqlx_repository_i32_from_u64(
                POSTGRES_BACKEND,
                u64::from(claim.attempt),
                "outbox claim attempt",
                INTEGER_STORAGE,
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
                  AND attempts = $8
                "#,
            )
            .bind(OutboxMessageStatus::Pending.as_str())
            .bind(now_epoch)
            .bind(empty_string_as_none(error))
            .bind(&claim.message_id)
            .bind(OutboxMessageStatus::InFlight.as_str())
            .bind(&claim.worker_id)
            .bind(now_epoch)
            .bind(sqlx_repository_i32_from_u64(
                POSTGRES_BACKEND,
                u64::from(claim.attempt),
                "outbox claim attempt",
                INTEGER_STORAGE,
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
                  AND attempts = $8
                "#,
            )
            .bind(OutboxMessageStatus::Failed.as_str())
            .bind(empty_string_as_none(error))
            .bind(now_epoch)
            .bind(&claim.message_id)
            .bind(OutboxMessageStatus::InFlight.as_str())
            .bind(&claim.worker_id)
            .bind(now_epoch)
            .bind(sqlx_repository_i32_from_u64(
                POSTGRES_BACKEND,
                u64::from(claim.attempt),
                "outbox claim attempt",
                INTEGER_STORAGE,
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

impl SnapshotStore for PostgresRepository {
    fn get_snapshot<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<Option<SnapshotRecord>, RepositoryError>> + Send + 'a {
        async move {
            let row = sqlx::query(
                r#"
                SELECT aggregate_type,
                       aggregate_id,
                       version,
                       snapshot_version,
                       payload,
                       payload_codec,
                       payload_codec_version,
                       metadata::text AS metadata,
                       EXTRACT(EPOCH FROM recorded_at)::double precision AS recorded_at_epoch
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

#[derive(Clone)]
struct IncludeSpec {
    name: String,
    relationship: RelationshipDef,
    target_schema: ReadModelSchema,
}

fn remember_read_model_schemas(
    stored: &RwLock<TableSchemaRegistry>,
    registry: &TableSchemaRegistry,
) -> Result<(), ReadModelError> {
    let mut stored = stored
        .write()
        .map_err(|_| ReadModelError::Storage("read-model schema registry lock poisoned".into()))?;

    for schema in registry.schemas() {
        if let Some(existing) = stored.schema_for_table(&schema.table_name) {
            if existing != schema {
                return Err(ReadModelError::Metadata(format!(
                    "read-model schema registry already contains table `{}` with different metadata",
                    schema.table_name
                )));
            }
            continue;
        }
        stored.register_schema(schema.clone())?;
    }

    Ok(())
}

fn resolve_registered_read_model_schemas(
    registry: &RwLock<TableSchemaRegistry>,
    request: &ReadModelLoadRequest,
) -> Result<(ReadModelSchema, Vec<IncludeSpec>), ReadModelError> {
    if request.includes.is_empty() {
        return Ok((request.schema.clone(), Vec::new()));
    }

    let registry = registry
        .read()
        .map_err(|_| ReadModelError::Storage("read-model schema registry lock poisoned".into()))?;
    let root_schema = registry
        .schema_for_model(&request.schema.model_name)
        .cloned()
        .ok_or_else(|| {
            ReadModelError::Metadata(format!(
                "read model `{}` is not registered for relationship includes",
                request.schema.model_name
            ))
        })?;
    if root_schema != request.schema {
        return Err(ReadModelError::Metadata(format!(
            "read model `{}` load request does not match registered schema",
            request.schema.model_name
        )));
    }

    let mut include_specs = Vec::with_capacity(request.includes.len());
    for include_name in &request.includes {
        let relationship = root_schema
            .relationships
            .iter()
            .find(|relationship| relationship.field_name == *include_name)
            .ok_or_else(|| {
                ReadModelError::Metadata(format!(
                    "read model `{}` has no relationship `{}`",
                    root_schema.model_name, include_name
                ))
            })?;
        if matches!(relationship.kind, RelationshipKind::ManyToMany) {
            return Err(ReadModelError::Metadata(format!(
                "many-to-many relationship `{}` includes are not supported until join metadata declares source and target keys",
                relationship.field_name
            )));
        }
        let target_schema = registry
            .schema_for_model(&relationship.target_model)
            .ok_or_else(|| {
                ReadModelError::Metadata(format!(
                    "read model `{}` relationship `{}` targets unregistered model `{}`",
                    root_schema.model_name, relationship.field_name, relationship.target_model
                ))
            })?;

        include_specs.push(IncludeSpec {
            name: include_name.clone(),
            relationship: relationship.clone(),
            target_schema: target_schema.clone(),
        });
    }

    Ok((root_schema, include_specs))
}

fn sql_read_model_capabilities() -> ReadModelAdapterCapabilities {
    ReadModelAdapterCapabilities {
        relational_rows: true,
        sparse_patches: true,
        deletes: true,
    }
}

fn validate_sql_write_plan(plan: &ReadModelWritePlan) -> Result<(), ReadModelError> {
    plan.validate_for(&sql_read_model_capabilities())
}

/// Record a consumer inbox receipt in the commit transaction. The
/// `(consumer, message_id)` primary key is the dedupe gate: a unique violation
/// means the message was already processed, so the whole batch rolls back and the
/// effects are not double-applied. `processed_at` defaults server-side.
async fn insert_inbox_receipt_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    receipt: &InboxReceipt,
) -> Result<(), RepositoryError> {
    receipt.validate()?;
    let result = sqlx::query("INSERT INTO consumer_inbox (consumer, message_id) VALUES ($1, $2)")
        .bind(&receipt.consumer)
        .bind(&receipt.message_id)
        .execute(&mut **tx)
        .await;
    match result {
        Ok(_) => Ok(()),
        Err(err) if is_postgres_unique_violation(&err) => {
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

async fn begin_read_model_tx(pool: &PgPool) -> Result<Transaction<'_, Postgres>, ReadModelError> {
    pool.begin()
        .await
        .map_err(|err| read_model_storage_error("begin transaction", err))
}

async fn commit_read_model_tx(tx: Transaction<'_, Postgres>) -> Result<(), ReadModelError> {
    tx.commit()
        .await
        .map_err(|err| read_model_storage_error("commit transaction", err))
}

async fn apply_read_model_write_plan_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    plan: ReadModelWritePlan,
) -> Result<ReadModelCommitOutcome, ReadModelError> {
    validate_sql_write_plan(&plan)?;

    for mutation in plan.mutations {
        match mutation {
            ReadModelMutation::UpsertRow(mutation) => {
                upsert_relational_row_in_tx(tx, mutation).await?;
            }
            ReadModelMutation::PatchRow(mutation) => {
                patch_relational_row_in_tx(tx, mutation).await?;
            }
            ReadModelMutation::DeleteRow(mutation) => {
                delete_relational_row_in_tx(tx, mutation).await?;
            }
        }
    }

    Ok(ReadModelCommitOutcome::applied())
}

async fn upsert_relational_row_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    mutation: RowMutation,
) -> Result<(), ReadModelError> {
    validate_key(&mutation.schema, &mutation.key)?;
    validate_row_values(&mutation.schema, &mutation.values, true)?;
    validate_values_match_key(&mutation.schema, &mutation.key, &mutation.values)?;

    let current_version = row_version_in_tx(tx, &mutation.schema, &mutation.key).await?;
    validate_row_expected_version(
        &mutation.schema,
        &mutation.key,
        &mutation.expected_version,
        current_version,
    )?;
    if matches!(mutation.mode, RowWriteMode::Insert) && current_version.is_some() {
        return Err(row_concurrency_conflict(
            &mutation.schema,
            &mutation.key,
            0,
            current_version.unwrap_or_default(),
        ));
    }

    match current_version {
        Some(expected_version) => {
            let new_version = next_row_version(&mutation.schema, &mutation.key, current_version)?;
            let rows_affected = update_relational_row_values_in_tx(
                tx,
                &mutation.schema,
                &mutation.key,
                &mutation.values,
                expected_version,
                new_version,
            )
            .await?;
            if rows_affected == 0 {
                let actual = row_version_in_tx(tx, &mutation.schema, &mutation.key)
                    .await?
                    .unwrap_or(expected_version);
                return Err(row_concurrency_conflict(
                    &mutation.schema,
                    &mutation.key,
                    expected_version,
                    actual,
                ));
            }
        }
        None => {
            insert_relational_row_in_tx(
                tx,
                &mutation.schema,
                &mutation.values,
                initial_row_version(),
            )
            .await?;
        }
    }

    Ok(())
}

async fn patch_relational_row_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    mutation: PatchRowMutation,
) -> Result<(), ReadModelError> {
    validate_key(&mutation.schema, &mutation.key)?;

    let current_version = row_version_in_tx(tx, &mutation.schema, &mutation.key).await?;
    validate_row_expected_version(
        &mutation.schema,
        &mutation.key,
        &mutation.expected_version,
        current_version,
    )?;

    match current_version {
        Some(expected_version) => {
            let patch_values =
                patch_values_preserving_key(&mutation.schema, &mutation.key, mutation.patch)?;
            let new_version = next_row_version(&mutation.schema, &mutation.key, current_version)?;
            let rows_affected = update_relational_patch_in_tx(
                tx,
                &mutation.schema,
                &mutation.key,
                patch_values,
                expected_version,
                new_version,
            )
            .await?;
            if rows_affected == 0 {
                let actual = row_version_in_tx(tx, &mutation.schema, &mutation.key)
                    .await?
                    .unwrap_or(expected_version);
                return Err(row_concurrency_conflict(
                    &mutation.schema,
                    &mutation.key,
                    expected_version,
                    actual,
                ));
            }
        }
        None if matches!(mutation.mode, PatchMode::InsertMissing) => {
            let values =
                row_values_from_key_and_patch(&mutation.schema, &mutation.key, mutation.patch)?;
            insert_relational_row_in_tx(tx, &mutation.schema, &values, initial_row_version())
                .await?;
        }
        None => {
            return Err(ReadModelError::NotFound {
                collection: mutation.schema.table_name,
                id: key_fingerprint(&mutation.key),
            });
        }
    }

    Ok(())
}

async fn delete_relational_row_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    mutation: DeleteRowMutation,
) -> Result<(), ReadModelError> {
    validate_key(&mutation.schema, &mutation.key)?;
    let current_version = row_version_in_tx(tx, &mutation.schema, &mutation.key).await?;
    validate_row_expected_version(
        &mutation.schema,
        &mutation.key,
        &mutation.expected_version,
        current_version,
    )?;

    let rows_affected = delete_relational_row_where_current_in_tx(
        tx,
        &mutation.schema,
        &mutation.key,
        current_version,
    )
    .await?;
    if rows_affected == 0 {
        if let Some(expected_version) = current_version {
            let actual = row_version_in_tx(tx, &mutation.schema, &mutation.key)
                .await?
                .unwrap_or(expected_version);
            return Err(row_concurrency_conflict(
                &mutation.schema,
                &mutation.key,
                expected_version,
                actual,
            ));
        }
    }

    Ok(())
}

async fn row_version_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    schema: &ReadModelSchema,
    key: &RowKey,
) -> Result<Option<u64>, ReadModelError> {
    let version_column = version_column(schema)?;
    let mut builder = QueryBuilder::<Postgres>::new("SELECT ");
    builder.push(quote_identifier(version_column));
    builder.push(" FROM ");
    builder.push(quote_identifier(&schema.table_name));
    push_postgres_key_predicates(&mut builder, schema, key)?;

    let row = builder
        .build()
        .fetch_optional(&mut **tx)
        .await
        .map_err(|err| read_model_storage_error("load relational row version", err))?;

    row.map(|row| {
        sqlx_read_model_u64_from_i64(
            POSTGRES_BACKEND,
            row.try_get::<i64, _>(version_column)
                .map_err(|err| read_model_storage_error("decode relational row version", err))?,
            version_column,
        )
    })
    .transpose()
}

async fn insert_relational_row_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    schema: &ReadModelSchema,
    values: &RowValues,
    version: u64,
) -> Result<(), ReadModelError> {
    let version_column = version_column(schema)?;
    let write_values = row_write_values(schema, values)?;
    let has_write_values = !write_values.is_empty();
    let mut builder = QueryBuilder::<Postgres>::new("INSERT INTO ");
    builder.push(quote_identifier(&schema.table_name));
    builder.push(" (");
    for (index, (column, _)) in write_values.iter().enumerate() {
        if index > 0 {
            builder.push(", ");
        }
        builder.push(quote_identifier(&column.column_name));
    }
    if has_write_values {
        builder.push(", ");
    }
    builder.push(quote_identifier(version_column));
    builder.push(") VALUES (");
    for (index, (column, value)) in write_values.into_iter().enumerate() {
        if index > 0 {
            builder.push(", ");
        }
        push_postgres_row_value_bind(&mut builder, value, column)?;
    }
    if has_write_values {
        builder.push(", ");
    }
    builder.push_bind(sqlx_read_model_i64_from_u64(
        POSTGRES_BACKEND,
        version,
        version_column,
        BIGINT_STORAGE,
    )?);
    builder.push(")");

    builder
        .build()
        .execute(&mut **tx)
        .await
        .map_err(|err| read_model_storage_error("insert relational row", err))?;

    Ok(())
}

async fn update_relational_row_values_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    schema: &ReadModelSchema,
    key: &RowKey,
    values: &RowValues,
    expected_version: u64,
    version: u64,
) -> Result<u64, ReadModelError> {
    let mut write_values = row_write_values(schema, values)?;
    write_values.retain(|(column, _)| !column.primary_key);
    update_relational_columns_in_tx(tx, schema, key, write_values, expected_version, version).await
}

async fn update_relational_patch_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    schema: &ReadModelSchema,
    key: &RowKey,
    write_values: Vec<(&ColumnDef, RowValue)>,
    expected_version: u64,
    version: u64,
) -> Result<u64, ReadModelError> {
    update_relational_columns_in_tx(tx, schema, key, write_values, expected_version, version).await
}

async fn update_relational_columns_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    schema: &ReadModelSchema,
    key: &RowKey,
    write_values: Vec<(&ColumnDef, RowValue)>,
    expected_version: u64,
    version: u64,
) -> Result<u64, ReadModelError> {
    let version_column = version_column(schema)?;
    let mut builder = QueryBuilder::<Postgres>::new("UPDATE ");
    builder.push(quote_identifier(&schema.table_name));
    builder.push(" SET ");
    let mut wrote_set = false;
    for (column, value) in write_values {
        if wrote_set {
            builder.push(", ");
        }
        builder.push(quote_identifier(&column.column_name));
        builder.push(" = ");
        push_postgres_row_value_bind(&mut builder, value, column)?;
        wrote_set = true;
    }
    if wrote_set {
        builder.push(", ");
    }
    builder.push(quote_identifier(version_column));
    builder.push(" = ");
    builder.push_bind(sqlx_read_model_i64_from_u64(
        POSTGRES_BACKEND,
        version,
        version_column,
        BIGINT_STORAGE,
    )?);
    push_postgres_key_predicates(&mut builder, schema, key)?;
    builder.push(" AND ");
    builder.push(quote_identifier(version_column));
    builder.push(" = ");
    builder.push_bind(sqlx_read_model_i64_from_u64(
        POSTGRES_BACKEND,
        expected_version,
        "expected version",
        BIGINT_STORAGE,
    )?);

    let result = builder
        .build()
        .execute(&mut **tx)
        .await
        .map_err(|err| read_model_storage_error("update relational row", err))?;
    Ok(result.rows_affected())
}

async fn delete_relational_row_where_current_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    schema: &ReadModelSchema,
    key: &RowKey,
    current_version: Option<u64>,
) -> Result<u64, ReadModelError> {
    let mut builder = QueryBuilder::<Postgres>::new("DELETE FROM ");
    builder.push(quote_identifier(&schema.table_name));
    push_postgres_key_predicates(&mut builder, schema, key)?;
    if let Some(version) = current_version {
        let version_column = version_column(schema)?;
        builder.push(" AND ");
        builder.push(quote_identifier(version_column));
        builder.push(" = ");
        builder.push_bind(sqlx_read_model_i64_from_u64(
            POSTGRES_BACKEND,
            version,
            "expected version",
            BIGINT_STORAGE,
        )?);
    }

    let result = builder
        .build()
        .execute(&mut **tx)
        .await
        .map_err(|err| read_model_storage_error("delete relational row", err))?;
    Ok(result.rows_affected())
}

async fn load_postgres_relational_row_by_key(
    pool: &PgPool,
    schema: &ReadModelSchema,
    key: &RowKey,
) -> Result<Option<Versioned<RowValues>>, ReadModelError> {
    validate_key(schema, key)?;
    let mut builder = postgres_relational_row_select(schema)?;
    push_postgres_key_predicates(&mut builder, schema, key)?;
    let row = builder
        .build()
        .fetch_optional(pool)
        .await
        .map_err(|err| read_model_storage_error("load relational row", err))?;
    row.map(|row| postgres_row_to_versioned_values(schema, &row))
        .transpose()
}

async fn load_postgres_relationship_rows(
    pool: &PgPool,
    root_schema: &ReadModelSchema,
    root_row: &RowValues,
    spec: &IncludeSpec,
) -> Result<Vec<Versioned<RowValues>>, ReadModelError> {
    match spec.relationship.kind {
        RelationshipKind::HasMany => {
            load_postgres_has_many_rows(pool, root_schema, root_row, spec).await
        }
        RelationshipKind::BelongsTo => {
            load_postgres_belongs_to_rows(pool, root_schema, root_row, spec).await
        }
        RelationshipKind::ManyToMany => Err(ReadModelError::Metadata(format!(
            "many-to-many relationship `{}` includes are not supported yet",
            spec.relationship.field_name
        ))),
    }
}

async fn load_postgres_has_many_rows(
    pool: &PgPool,
    root_schema: &ReadModelSchema,
    root_row: &RowValues,
    spec: &IncludeSpec,
) -> Result<Vec<Versioned<RowValues>>, ReadModelError> {
    let foreign_key = spec.relationship.foreign_key.as_deref().ok_or_else(|| {
        ReadModelError::Metadata(format!(
            "relationship `{}` must declare a foreign key",
            spec.relationship.field_name
        ))
    })?;
    let target_column = column_name_for(&spec.target_schema, foreign_key).ok_or_else(|| {
        ReadModelError::Metadata(format!(
            "relationship `{}` foreign key `{}` is not a target column",
            spec.relationship.field_name, foreign_key
        ))
    })?;
    let root_column = column_name_for(root_schema, foreign_key)
        .or_else(|| root_schema.primary_key.columns.first().cloned())
        .ok_or_else(|| {
            ReadModelError::Metadata(format!(
                "relationship `{}` has no root key column",
                spec.relationship.field_name
            ))
        })?;
    let root_value = root_row.get(&root_column).ok_or_else(|| {
        ReadModelError::Metadata(format!(
            "read model `{}` root row is missing relationship key `{}`",
            root_schema.model_name, root_column
        ))
    })?;

    load_postgres_rows_matching_column(pool, &spec.target_schema, &target_column, root_value).await
}

async fn load_postgres_belongs_to_rows(
    pool: &PgPool,
    root_schema: &ReadModelSchema,
    root_row: &RowValues,
    spec: &IncludeSpec,
) -> Result<Vec<Versioned<RowValues>>, ReadModelError> {
    let foreign_key = spec.relationship.foreign_key.as_deref().ok_or_else(|| {
        ReadModelError::Metadata(format!(
            "relationship `{}` must declare a foreign key",
            spec.relationship.field_name
        ))
    })?;
    let source_column = column_name_for(root_schema, foreign_key).ok_or_else(|| {
        ReadModelError::Metadata(format!(
            "relationship `{}` foreign key `{}` is not a source column",
            spec.relationship.field_name, foreign_key
        ))
    })?;
    let target_column = belongs_to_target_column(&spec.target_schema, &source_column)?;
    let source_value = root_row.get(&source_column).ok_or_else(|| {
        ReadModelError::Metadata(format!(
            "read model `{}` root row is missing relationship key `{}`",
            root_schema.model_name, source_column
        ))
    })?;

    load_postgres_rows_matching_column(pool, &spec.target_schema, &target_column, source_value)
        .await
}

async fn load_postgres_rows_matching_column(
    pool: &PgPool,
    schema: &ReadModelSchema,
    column_name: &str,
    value: &RowValue,
) -> Result<Vec<Versioned<RowValues>>, ReadModelError> {
    let column = column_by_name(schema, column_name)?;
    let mut builder = postgres_relational_row_select(schema)?;
    builder.push(" WHERE ");
    builder.push(quote_identifier(column_name));
    builder.push(" = ");
    push_postgres_row_value_bind(&mut builder, value.clone(), column)?;
    push_postgres_order_by_primary_key(&mut builder, schema);

    let rows = builder
        .build()
        .fetch_all(pool)
        .await
        .map_err(|err| read_model_storage_error("load relationship rows", err))?;

    rows.iter()
        .map(|row| postgres_row_to_versioned_values(schema, row))
        .collect()
}

fn postgres_relational_row_select(
    schema: &ReadModelSchema,
) -> Result<QueryBuilder<Postgres>, ReadModelError> {
    let version_column = version_column(schema)?;
    let mut builder = QueryBuilder::<Postgres>::new("SELECT ");
    for (index, column) in schema.columns.iter().enumerate() {
        if index > 0 {
            builder.push(", ");
        }
        push_postgres_select_column(&mut builder, column);
    }
    if !schema.columns.is_empty() {
        builder.push(", ");
    }
    builder.push(quote_identifier(version_column));
    builder.push(" FROM ");
    builder.push(quote_identifier(&schema.table_name));
    Ok(builder)
}

fn push_postgres_select_column(builder: &mut QueryBuilder<Postgres>, column: &ColumnDef) {
    builder.push(quote_identifier(&column.column_name));
    if matches!(column.column_type, ColumnType::Json | ColumnType::Timestamp) {
        builder.push("::text");
    }
    builder.push(" AS ");
    builder.push(quote_identifier(&column.column_name));
}

fn push_postgres_order_by_primary_key(
    builder: &mut QueryBuilder<Postgres>,
    schema: &ReadModelSchema,
) {
    if schema.primary_key.columns.is_empty() {
        return;
    }
    builder.push(" ORDER BY ");
    for (index, column) in schema.primary_key.columns.iter().enumerate() {
        if index > 0 {
            builder.push(", ");
        }
        builder.push(quote_identifier(column));
    }
}

fn postgres_row_to_versioned_values(
    schema: &ReadModelSchema,
    row: &PgRow,
) -> Result<Versioned<RowValues>, ReadModelError> {
    let mut values = RowValues::new();
    for column in &schema.columns {
        values.insert(column.column_name.clone(), postgres_row_value(row, column)?);
    }
    let version_column = version_column(schema)?;
    let version = sqlx_read_model_u64_from_i64(
        POSTGRES_BACKEND,
        row.try_get::<i64, _>(version_column)
            .map_err(|err| read_model_storage_error("decode relational row version", err))?,
        version_column,
    )?;
    Ok(Versioned {
        data: values,
        version,
    })
}

fn postgres_row_value(row: &PgRow, column: &ColumnDef) -> Result<RowValue, ReadModelError> {
    Ok(match column.column_type {
        ColumnType::Text | ColumnType::Timestamp => row
            .try_get::<Option<String>, _>(column.column_name.as_str())
            .map_err(|err| read_model_storage_error("decode relational text column", err))?
            .map(RowValue::String)
            .unwrap_or(RowValue::Null),
        ColumnType::Boolean => row
            .try_get::<Option<bool>, _>(column.column_name.as_str())
            .map_err(|err| read_model_storage_error("decode relational boolean column", err))?
            .map(RowValue::Bool)
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
                sqlx_read_model_u64_from_i64(POSTGRES_BACKEND, value, column.column_name.as_str())
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

fn belongs_to_target_column(
    target_schema: &ReadModelSchema,
    source_column: &str,
) -> Result<String, ReadModelError> {
    if target_schema.primary_key.columns.len() != 1 {
        return Err(ReadModelError::Metadata(format!(
            "belongs_to target `{}` must have a single-column primary key to load from `{}`",
            target_schema.model_name, source_column
        )));
    }

    Ok(target_schema.primary_key.columns[0].clone())
}

fn initial_row_version() -> u64 {
    1
}

fn next_row_version(
    schema: &ReadModelSchema,
    key: &RowKey,
    current_version: Option<u64>,
) -> Result<u64, ReadModelError> {
    match current_version {
        Some(version) => version.checked_add(1).ok_or_else(|| {
            ReadModelError::Storage(format!(
                "read model version overflow for {}:{}",
                schema.table_name,
                key_fingerprint(key)
            ))
        }),
        None => Ok(initial_row_version()),
    }
}

fn validate_row_expected_version(
    schema: &ReadModelSchema,
    key: &RowKey,
    expected_version: &ExpectedVersion,
    current_version: Option<u64>,
) -> Result<(), ReadModelError> {
    match (expected_version, current_version) {
        (ExpectedVersion::Any, _) => Ok(()),
        (ExpectedVersion::Exact(expected), Some(actual)) if expected == &actual => Ok(()),
        (ExpectedVersion::Exact(expected), Some(actual)) => {
            Err(row_concurrency_conflict(schema, key, *expected, actual))
        }
        (ExpectedVersion::Exact(_), None) => Err(ReadModelError::NotFound {
            collection: schema.table_name.clone(),
            id: key_fingerprint(key),
        }),
        (ExpectedVersion::NotExists, None) => Ok(()),
        (ExpectedVersion::NotExists, Some(actual)) => {
            Err(row_concurrency_conflict(schema, key, 0, actual))
        }
    }
}

fn row_concurrency_conflict(
    schema: &ReadModelSchema,
    key: &RowKey,
    expected: u64,
    actual: u64,
) -> ReadModelError {
    ReadModelError::ConcurrencyConflict {
        collection: schema.table_name.clone(),
        id: key_fingerprint(key),
        expected,
        actual,
    }
}

fn row_values_from_key_and_patch(
    schema: &ReadModelSchema,
    key: &RowKey,
    patch: crate::read_model::RowPatch,
) -> Result<RowValues, ReadModelError> {
    let mut values = RowValues::new();
    for (column, value) in key.iter() {
        values.insert(column.to_string(), value.clone());
    }
    for (column, value) in patch.into_values() {
        if schema
            .primary_key
            .columns
            .iter()
            .any(|primary_key| primary_key == &column)
        {
            let key_value = key.get(&column).ok_or_else(|| {
                ReadModelError::Metadata(format!(
                    "read model `{}` row key is missing primary-key column `{}`",
                    schema.model_name, column
                ))
            })?;
            if key_value != &value {
                return Err(ReadModelError::Metadata(format!(
                    "read model `{}` patch cannot change primary-key column `{}`",
                    schema.model_name, column
                )));
            }
        }
        values.insert(column, value);
    }
    validate_row_values(schema, &values, true)?;
    validate_values_match_key(schema, key, &values)?;
    Ok(values)
}

fn patch_values_preserving_key<'schema>(
    schema: &'schema ReadModelSchema,
    key: &RowKey,
    patch: crate::read_model::RowPatch,
) -> Result<Vec<(&'schema ColumnDef, RowValue)>, ReadModelError> {
    let mut values = Vec::new();
    for (column_name, value) in patch.into_values() {
        let column = column_by_name(schema, &column_name)?;
        if column.primary_key {
            let key_value = key.get(&column_name).ok_or_else(|| {
                ReadModelError::Metadata(format!(
                    "read model `{}` row key is missing primary-key column `{}`",
                    schema.model_name, column_name
                ))
            })?;
            if key_value != &value {
                return Err(ReadModelError::Metadata(format!(
                    "read model `{}` patch cannot change primary-key column `{}`",
                    schema.model_name, column_name
                )));
            }
            continue;
        }
        values.push((column, value));
    }
    Ok(values)
}

fn validate_values_match_key(
    schema: &ReadModelSchema,
    key: &RowKey,
    values: &RowValues,
) -> Result<(), ReadModelError> {
    for column in &schema.primary_key.columns {
        let key_value = key.get(column).ok_or_else(|| {
            ReadModelError::Metadata(format!(
                "read model `{}` row key is missing primary-key column `{}`",
                schema.model_name, column
            ))
        })?;
        let row_value = values.get(column).ok_or_else(|| {
            ReadModelError::Metadata(format!(
                "read model `{}` row is missing primary-key column `{}`",
                schema.model_name, column
            ))
        })?;
        if row_value != key_value {
            return Err(ReadModelError::Metadata(format!(
                "read model `{}` row values cannot change primary-key column `{}`",
                schema.model_name, column
            )));
        }
    }
    Ok(())
}

fn row_write_values<'schema>(
    schema: &'schema ReadModelSchema,
    values: &RowValues,
) -> Result<Vec<(&'schema ColumnDef, RowValue)>, ReadModelError> {
    values
        .iter()
        .map(|(column_name, value)| Ok((column_by_name(schema, column_name)?, value.clone())))
        .collect()
}

fn column_by_name<'schema>(
    schema: &'schema ReadModelSchema,
    column_name: &str,
) -> Result<&'schema ColumnDef, ReadModelError> {
    schema
        .columns
        .iter()
        .find(|column| column.column_name == column_name)
        .ok_or_else(|| {
            ReadModelError::Metadata(format!(
                "read model `{}` write references missing column `{}`",
                schema.model_name, column_name
            ))
        })
}

fn version_column(schema: &ReadModelSchema) -> Result<&str, ReadModelError> {
    schema.version_column.as_deref().ok_or_else(|| {
        ReadModelError::Metadata(format!(
            "read model `{}` requires a version column for SQL write-plan persistence",
            schema.model_name
        ))
    })
}

fn push_postgres_key_predicates(
    builder: &mut QueryBuilder<Postgres>,
    schema: &ReadModelSchema,
    key: &RowKey,
) -> Result<(), ReadModelError> {
    builder.push(" WHERE ");
    for (index, column_name) in schema.primary_key.columns.iter().enumerate() {
        if index > 0 {
            builder.push(" AND ");
        }
        let column = column_by_name(schema, column_name)?;
        let value = key.get(column_name).cloned().ok_or_else(|| {
            ReadModelError::Metadata(format!(
                "read model `{}` row key is missing primary-key column `{}`",
                schema.model_name, column_name
            ))
        })?;
        builder.push(quote_identifier(column_name));
        builder.push(" = ");
        push_postgres_row_value_bind(builder, value, column)?;
    }
    Ok(())
}

fn push_postgres_row_value_bind(
    builder: &mut QueryBuilder<Postgres>,
    value: RowValue,
    column: &ColumnDef,
) -> Result<(), ReadModelError> {
    match value {
        RowValue::Null => push_postgres_null_bind(builder, column)?,
        RowValue::Bool(value) => {
            builder.push_bind(value);
        }
        RowValue::I64(value) => {
            builder.push_bind(value);
        }
        RowValue::U64(value) => {
            builder.push_bind(sqlx_read_model_i64_from_u64(
                POSTGRES_BACKEND,
                value,
                &column.column_name,
                BIGINT_STORAGE,
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
    push_postgres_type_cast(builder, column);
    Ok(())
}

fn push_postgres_null_bind(
    builder: &mut QueryBuilder<Postgres>,
    column: &ColumnDef,
) -> Result<(), ReadModelError> {
    match &column.column_type {
        ColumnType::Text | ColumnType::Json | ColumnType::Timestamp => {
            builder.push_bind(Option::<String>::None);
        }
        ColumnType::Boolean => {
            builder.push_bind(Option::<bool>::None);
        }
        ColumnType::Integer | ColumnType::UnsignedInteger => {
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
                "read model `{}` column `{}` has unsupported type `{}`",
                column.field_name, column.column_name, type_name
            )));
        }
    }
    Ok(())
}

fn push_postgres_type_cast(builder: &mut QueryBuilder<Postgres>, column: &ColumnDef) {
    match column.column_type {
        ColumnType::Json => {
            builder.push("::jsonb");
        }
        ColumnType::Timestamp => {
            builder.push("::timestamptz");
        }
        _ => {}
    }
}

fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
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

/// Insert every event across all prepared appends in a single multi-row INSERT.
///
/// Conflict detection is unchanged from the per-row path: the `(aggregate_type,
/// aggregate_id, sequence)` primary key is the contiguity gate, and a unique
/// violation still surfaces as `ConcurrentWrite`. Because a failed statement
/// aborts the transaction, the conflicting stream's actual version is re-read
/// over the pool (a separate connection), exactly as the per-row path did.
async fn insert_events_in_tx(
    pool: &PgPool,
    tx: &mut Transaction<'_, Postgres>,
    prepared: &[PreparedEventAppend],
) -> Result<(), RepositoryError> {
    if prepared.iter().all(|append| append.events.is_empty()) {
        return Ok(());
    }

    // Each row carries pre-validated bind values; build them before the query so
    // any conversion error surfaces before we touch the database.
    struct EventRow<'a> {
        aggregate_type: &'a str,
        aggregate_id: &'a str,
        sequence: i64,
        event_name: &'a str,
        event_version: i32,
        payload: &'a [u8],
        payload_codec: &'a str,
        payload_codec_version: i32,
        metadata: String,
        recorded_at: f64,
    }

    let mut rows = Vec::new();
    for append in prepared {
        for event in &append.events {
            rows.push(EventRow {
                aggregate_type: append.identity.aggregate_type(),
                aggregate_id: append.identity.aggregate_id(),
                sequence: sqlx_repository_i64_from_u64(
                    POSTGRES_BACKEND,
                    event.sequence,
                    "sequence",
                    BIGINT_STORAGE,
                )?,
                event_name: &event.event_name,
                event_version: sqlx_repository_i32_from_u64(
                    POSTGRES_BACKEND,
                    event.event_version,
                    "event_version",
                    INTEGER_STORAGE,
                )?,
                payload: &event.payload,
                payload_codec: &event.payload_codec,
                payload_codec_version: i32::from(event.payload_codec_version),
                metadata: serialize_event_metadata(&event.metadata)?,
                recorded_at: system_time_to_epoch_secs(event.timestamp)?,
            });
        }
    }

    let mut builder = QueryBuilder::<Postgres>::new(
        "INSERT INTO aggregate_events (\
         aggregate_type, aggregate_id, sequence, event_name, event_version, \
         payload, payload_codec, payload_codec_version, metadata, recorded_at) ",
    );
    builder.push_values(rows, |mut row, event| {
        row.push_bind(event.aggregate_type)
            .push_bind(event.aggregate_id)
            .push_bind(event.sequence)
            .push_bind(event.event_name)
            .push_bind(event.event_version)
            .push_bind(event.payload)
            .push_bind(event.payload_codec)
            .push_bind(event.payload_codec_version)
            .push_bind(event.metadata)
            .push_unseparated("::jsonb");
        // recorded_at is stored from epoch seconds via to_timestamp(...).
        row.push("to_timestamp(")
            .push_bind_unseparated(event.recorded_at)
            .push_unseparated(")");
    });

    let result = builder.build().execute(&mut **tx).await;

    match result {
        Ok(_) => Ok(()),
        Err(err) if is_postgres_unique_violation(&err) => {
            Err(concurrent_write_from_conflict(pool, prepared).await)
        }
        Err(err) => Err(repository_storage_error("insert events", err)),
    }
}

/// After an event-insert unique violation, find the stream whose actual version
/// no longer matches its expected version and report it as `ConcurrentWrite`.
/// Falls back to the first append if a concurrent writer's effect cannot be
/// pinned down (the violation still indicates a conflicting write).
async fn concurrent_write_from_conflict(
    pool: &PgPool,
    prepared: &[PreparedEventAppend],
) -> RepositoryError {
    for append in prepared {
        match stream_version_pool(pool, &append.identity).await {
            Ok(actual) if actual != append.expected_version => {
                return RepositoryError::ConcurrentWrite {
                    id: append.identity.to_string(),
                    expected: append.expected_version,
                    actual,
                };
            }
            Ok(_) => {}
            Err(err) => return err,
        }
    }

    let append = &prepared[0];
    match stream_version_pool(pool, &append.identity).await {
        Ok(actual) => RepositoryError::ConcurrentWrite {
            id: append.identity.to_string(),
            expected: append.expected_version,
            actual,
        },
        Err(err) => err,
    }
}

/// Insert every outbox message in a single multi-row INSERT. A unique violation
/// on `message_id` still maps to `DuplicateOutboxMessageInBatch`.
async fn insert_outbox_messages_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    messages: &[OutboxMessage],
) -> Result<(), RepositoryError> {
    if messages.is_empty() {
        return Ok(());
    }

    struct OutboxRow<'a> {
        message_id: &'a str,
        event_type: &'a str,
        payload: &'a [u8],
        payload_codec: &'a str,
        payload_codec_version: i32,
        destination: Option<&'a str>,
        metadata: String,
        status: &'a str,
        created_at: f64,
        worker_id: Option<&'a str>,
        leased_until: Option<f64>,
        attempts: i32,
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
            payload_codec_version: i32::from(message.payload_codec_version),
            destination: message.destination.as_deref(),
            metadata: serialize_event_metadata(&message.metadata)?,
            status: message.status.as_str(),
            created_at: system_time_to_epoch_secs(message.created_at)?,
            worker_id: message.worker_id.as_deref(),
            leased_until: message
                .leased_until
                .map(system_time_to_epoch_secs)
                .transpose()?,
            attempts: sqlx_repository_i32_from_u64(
                POSTGRES_BACKEND,
                u64::from(message.attempts),
                "outbox attempts",
                INTEGER_STORAGE,
            )?,
            last_error: message.last_error.as_deref(),
            source_aggregate_type: message.source_aggregate_type.as_deref(),
            source_aggregate_id: message.source_aggregate_id.as_deref(),
            source_sequence: message
                .source_sequence
                .map(|value| {
                    sqlx_repository_i64_from_u64(
                        POSTGRES_BACKEND,
                        value,
                        "outbox source sequence",
                        BIGINT_STORAGE,
                    )
                })
                .transpose()?,
            correlation_id: message.correlation_id(),
            causation_id: message.causation_id(),
        });
    }

    let mut builder = QueryBuilder::<Postgres>::new(
        "INSERT INTO outbox_messages (\
         message_id, event_type, payload, payload_codec, payload_codec_version, \
         destination, metadata, status, created_at, next_available_at, \
         claimed_by, claimed_until, attempts, last_error, source_aggregate_type, \
         source_aggregate_id, source_sequence, correlation_id, causation_id) ",
    );
    builder.push_values(rows, |mut row, message| {
        row.push_bind(message.message_id)
            .push_bind(message.event_type)
            .push_bind(message.payload)
            .push_bind(message.payload_codec)
            .push_bind(message.payload_codec_version)
            .push_bind(message.destination)
            .push_bind(message.metadata)
            .push_unseparated("::jsonb")
            .push_bind(message.status);
        // created_at and next_available_at share the same epoch-seconds value.
        row.push("to_timestamp(")
            .push_bind_unseparated(message.created_at)
            .push_unseparated(")");
        row.push("to_timestamp(")
            .push_bind_unseparated(message.created_at)
            .push_unseparated(")");
        row.push_bind(message.worker_id);
        // claimed_until: NULL stays NULL through to_timestamp, matching the
        // per-row path's `to_timestamp($n::double precision)`.
        row.push("to_timestamp(")
            .push_bind_unseparated(message.leased_until)
            .push_unseparated("::double precision)");
        row.push_bind(message.attempts)
            .push_bind(message.last_error)
            .push_bind(message.source_aggregate_type)
            .push_bind(message.source_aggregate_id)
            .push_bind(message.source_sequence)
            .push_bind(message.correlation_id)
            .push_bind(message.causation_id);
    });

    let result = builder.build().execute(&mut **tx).await;

    match result {
        Ok(_) => Ok(()),
        Err(err) if is_postgres_unique_violation(&err) => {
            // The batch was already deduped (reject_duplicate_outbox_messages),
            // so a violation means the id collides with a previously committed
            // row. Report the first message id as the offender, matching the
            // per-row path's contract.
            Err(RepositoryError::DuplicateOutboxMessageInBatch {
                id: messages[0].id().to_string(),
            })
        }
        Err(err) => Err(repository_storage_error("insert outbox messages", err)),
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
    let mut metadata = deserialize_event_metadata(&metadata_json)?;
    if let Some(correlation_id) = row
        .try_get::<Option<String>, _>("correlation_id")
        .map_err(|err| repository_storage_error("decode outbox correlation_id row", err))?
    {
        metadata.insert("correlation_id".into(), correlation_id);
    }
    if let Some(causation_id) = row
        .try_get::<Option<String>, _>("causation_id")
        .map_err(|err| repository_storage_error("decode outbox causation_id row", err))?
    {
        metadata.insert("causation_id".into(), causation_id);
    }

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
        metadata,
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

fn entity_from_events(aggregate_id: String, events: Vec<EventRecord>) -> Entity {
    let mut entity = Entity::new();
    entity.set_id(aggregate_id);
    entity.load_from_history(events);
    entity
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
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8::jsonb, to_timestamp($9))
        ON CONFLICT(aggregate_type, aggregate_id) DO UPDATE SET
          version = excluded.version,
          snapshot_version = excluded.snapshot_version,
          payload = excluded.payload,
          payload_codec = excluded.payload_codec,
          payload_codec_version = excluded.payload_codec_version,
          metadata = excluded.metadata,
          recorded_at = excluded.recorded_at,
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
    .bind(sqlx_repository_i32_from_u64(
        POSTGRES_BACKEND,
        record.snapshot_version,
        "snapshot payload version",
        INTEGER_STORAGE,
    )?)
    .bind(&record.payload)
    .bind(&record.payload_codec)
    .bind(i32::from(record.payload_codec_version))
    .bind(serialize_event_metadata(&record.metadata)?)
    .bind(system_time_to_epoch_secs(record.recorded_at)?)
    .execute(&mut **tx)
    .await
    .map_err(|err| repository_storage_error("save snapshot", err))?;

    Ok(())
}

fn snapshot_from_row(row: PgRow) -> Result<SnapshotRecord, RepositoryError> {
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
            POSTGRES_BACKEND,
            row.try_get("version")
                .map_err(|err| repository_storage_error("decode snapshot version row", err))?,
            "snapshot version",
        )?,
        snapshot_version: sqlx_repository_u64_from_i32(
            POSTGRES_BACKEND,
            row.try_get("snapshot_version").map_err(|err| {
                repository_storage_error("decode snapshot payload version row", err)
            })?,
            "snapshot payload version",
        )?,
        payload_codec: row
            .try_get("payload_codec")
            .map_err(|err| repository_storage_error("decode snapshot payload codec row", err))?,
        payload_codec_version: sqlx_repository_u16_from_i32(
            POSTGRES_BACKEND,
            row.try_get("payload_codec_version").map_err(|err| {
                repository_storage_error("decode snapshot payload codec version row", err)
            })?,
            "snapshot payload codec version",
        )?,
        payload: row
            .try_get("payload")
            .map_err(|err| repository_storage_error("decode snapshot payload row", err))?,
        metadata: deserialize_event_metadata(&metadata_json)?,
        recorded_at: system_time_from_epoch_secs(
            row.try_get("recorded_at_epoch")
                .map_err(|err| repository_storage_error("decode snapshot recorded_at row", err))?,
        )?,
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

fn read_model_storage_error(operation: &str, err: sqlx::Error) -> ReadModelError {
    sqlx_repo::read_model_storage_error(POSTGRES_BACKEND, operation, err)
}

fn table_schema_storage_error(operation: &str, err: sqlx::Error) -> TableStoreError {
    TableStoreError::Storage(format!("{POSTGRES_BACKEND} {operation} failed: {err}"))
}
