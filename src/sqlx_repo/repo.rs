//! Backend-agnostic event-store, snapshot, outbox, and inbox logic shared by
//! the Postgres and SQLite repositories.
//!
//! This extends the [`SqlxReadModelBackend`](super::read_model::SqlxReadModelBackend)
//! pattern to the whole repository surface: the SQL statements and row codecs
//! are identical across the two backends because `QueryBuilder<DB>` renders the
//! right placeholder dialect. What genuinely differs — schema SQL, bind-param
//! chunking, the timestamp codec (Postgres epoch-`f64`/`to_timestamp()` vs
//! SQLite `"secs.nanos"` text), the unique-violation predicate, conflict
//! recovery (SQLite can re-read in the failed transaction; a failed Postgres
//! statement aborts it), and the outbox `claim` strategy — lives behind the
//! [`SqlxRepoBackend`] trait, implemented once per backend.

#![expect(
    clippy::manual_async_fn,
    reason = "async trait impls return impl Future + Send to preserve public Send bounds"
)]

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sqlx::migrate::{Migrate, Migration, MigrationType, Migrator};
use sqlx::pool::PoolOptions;
use sqlx::query_builder::Separated;
use sqlx::{Encode, Executor, IntoArguments, Pool, QueryBuilder, Row, Transaction, Type};

use crate::entity::{Entity, EventRecord, BITCODE_PAYLOAD_CODEC};
use crate::outbox::{OutboxMessage, OutboxMessageStatus};
use crate::outbox_worker::{ensure_active_claim, ClaimOutboxMessages, OutboxClaimRef, OutboxStore};
use crate::read_model::{ReadModelLoadGraph, ReadModelLoadRequest, ReadModelQueryCapabilities};
use crate::repository::{
    validate_commit_batch, validate_snapshot_identity, validate_supported_event_codec, CommitBatch,
    GetStream, InboxReceipt, InboxStore, PreparedEventAppend, ReadModelWritePlanStore,
    RelationalReadModelQueryStore, RepositoryError, SnapshotStore, SnapshotWrite, StreamIdentity,
    TransactionalCommit,
};
use crate::snapshot::SnapshotRecord;
use crate::sqlx_repo::read_model::{
    apply_read_model_write_plan_in_tx, commit_read_model_write_plan, empty_string_as_none,
    load_read_model_graph, remember_read_model_schemas, sql_read_model_capabilities,
    validate_sql_write_plan, SqlxReadModelBackend,
};
use crate::sqlx_repo::{
    audited_table_schema_sql, deserialize_event_metadata, repository_i64_from_u64,
    repository_u16_from_i64, repository_u64_from_i64, serialize_event_metadata,
};
use crate::table::{
    generate_table_migration_artifacts, table_schema_bootstrap_result, table_schema_statements,
    TableMigrationArtifact, TableSchemaBootstrap, TableSchemaRegistry, TableSqlDialect,
    TableSqlSchemaAdapter, TableStoreError,
};
use crate::table::{
    TableAdapterCapabilities as ReadModelAdapterCapabilities,
    TableCommitOutcome as ReadModelCommitOutcome, TableStoreError as ReadModelError,
    TableWritePlan as ReadModelWritePlan,
};

/// Build an embedded migrator from statically included migration files
/// (`(version, description, sql)` per file, in order). sqlx's `migrate!`
/// macro would assemble this at compile time but drags in the whole
/// proc-macro stack; here the checksums are computed once at first use, so
/// keep each backend's list in sync with its `migrations/` directory.
pub(crate) fn embedded_migrator(files: &[(i64, &'static str, &'static str)]) -> Migrator {
    Migrator::with_migrations(
        files
            .iter()
            .map(|&(version, description, sql)| {
                Migration::new(
                    version,
                    description.into(),
                    MigrationType::Simple,
                    sqlx::SqlSafeStr::into_sql_str(sql),
                    false,
                )
            })
            .collect(),
    )
}

/// Group stream identities by aggregate type so each type is one id-list
/// round trip instead of a query per identity. Callers issue single-type
/// batches in the common case, so this usually yields one group; the grouping
/// only exists to keep arbitrary mixed-type inputs correct.
fn ids_by_type(identities: &[StreamIdentity]) -> BTreeMap<&str, Vec<&str>> {
    let mut groups: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for identity in identities {
        groups
            .entry(identity.aggregate_type())
            .or_default()
            .push(identity.aggregate_id());
    }
    groups
}

/// Bound parameters per `aggregate_events` row.
const EVENT_BIND_COLUMNS: usize = 10;

/// Bound parameters per `outbox_messages` row.
const OUTBOX_BIND_COLUMNS: usize = 19;

/// Dialect surface for the shared repository path (event store, snapshots,
/// outbox lifecycle, consumer inbox, schema bootstrap).
///
/// Everything the two SQL backends genuinely disagree on is an item here; the
/// free functions and the [`SqlxRepository`]/[`SqlxOutboxStore`] impls in this
/// module are the single copy of everything they agree on.
pub trait SqlxRepoBackend: SqlxReadModelBackend {
    /// Embedded migrations applied by `migrate_pool`. Runs through
    /// `sqlx::migrate::Migrator`, which keeps a `_sqlx_migrations` ledger and
    /// executes each migration file whole (the previous hand-rolled runner
    /// split files on `;`, which breaks on function bodies and string
    /// literals, and kept no record of what had been applied).
    fn migrator() -> &'static Migrator;
    /// Maximum bound parameters per statement. Multi-row inserts are chunked to
    /// stay under this; Postgres is effectively unlimited so chunking collapses
    /// to a single statement and both backends share one code path.
    const MAX_BIND_PARAMS: usize;
    /// Whether event-insert conflict recovery re-reads stream versions inside
    /// the failed transaction. SQLite does not abort a transaction on a
    /// constraint error, so the re-read can (and must, to see this tx's own
    /// earlier chunks) happen in-tx. A failed Postgres statement aborts the
    /// transaction, so the re-read runs on a separate pool connection.
    const CONFLICT_REREAD_IN_TX: bool;
    /// SQL expression producing the database's current timestamp, used for
    /// server-side `updated_at` maintenance.
    const NOW: &'static str;
    /// `SELECT` list for `aggregate_events` rows. The recorded-at column must
    /// surface as `recorded_at` in whatever representation
    /// [`decode_timestamp`](Self::decode_timestamp) reads.
    const EVENT_SELECT: &'static str;
    /// `SELECT` list for `aggregate_snapshots` rows (recorded-at as above).
    const SNAPSHOT_SELECT: &'static str;
    /// `SELECT` list for `outbox_messages` rows (`created_at`/`claimed_until`
    /// in the representation `decode_timestamp` reads).
    const OUTBOX_SELECT: &'static str;
    /// ORDER BY expression for outbox `created_at` ordering (SQLite stores
    /// timestamps as text and must cast for numeric ordering).
    const ORDER_BY_CREATED_AT: &'static str;
    /// Dialect for table/read-model schema artifact generation.
    const TABLE_DIALECT: TableSqlDialect;

    /// Owned bind value for a stored timestamp (Postgres: epoch seconds `f64`;
    /// SQLite: `"secs.nanos"` text).
    type TimestampValue: Send + Sync + 'static;

    /// Pool size when connecting from a database URL.
    fn default_pool_size(database_url: &str) -> u32 {
        let _ = database_url;
        5
    }

    /// Whether a `sqlx::Error` is this backend's unique-violation error.
    fn is_unique_violation(err: &sqlx::Error) -> bool;

    /// Encode a [`SystemTime`] into this backend's stored representation.
    fn timestamp_value(timestamp: SystemTime) -> Result<Self::TimestampValue, RepositoryError>;

    /// Push a timestamp value into a separated bind list (Postgres wraps the
    /// bind in `to_timestamp(...)`).
    fn push_timestamp(sep: &mut Separated<'_, Self, &'static str>, value: &Self::TimestampValue);

    /// Push an optional timestamp value into a separated bind list, binding a
    /// typed `NULL` when absent.
    fn push_optional_timestamp(
        sep: &mut Separated<'_, Self, &'static str>,
        value: Option<&Self::TimestampValue>,
    );

    /// Push `<assignment target> = <timestamp>` right-hand side into a builder.
    fn push_timestamp_assign(builder: &mut QueryBuilder<Self>, value: &Self::TimestampValue);

    /// Push `<column> <op> <now>` comparing a stored timestamp column against
    /// epoch seconds (Postgres: `column op to_timestamp($n)`; SQLite:
    /// `CAST(column AS REAL) op ?`).
    fn push_timestamp_cmp(
        builder: &mut QueryBuilder<Self>,
        column: &'static str,
        op: &'static str,
        epoch_secs: f64,
    );

    /// Decode a stored timestamp column into a [`SystemTime`].
    fn decode_timestamp(
        row: &Self::Row,
        column: &'static str,
    ) -> Result<SystemTime, RepositoryError>;

    /// Decode a nullable stored timestamp column.
    fn decode_optional_timestamp(
        row: &Self::Row,
        column: &'static str,
    ) -> Result<Option<SystemTime>, RepositoryError>;

    /// Push a metadata JSON bind (Postgres casts to `::jsonb`).
    fn push_metadata(sep: &mut Separated<'_, Self, &'static str>, json: &str);

    /// Push an `aggregate_id` filter for an id list (Postgres: `= ANY($n)`
    /// array bind; SQLite: `IN (?, ?, ...)`).
    fn push_id_filter(builder: &mut QueryBuilder<Self>, ids: &[&str]);

    /// Build the consumer-inbox retention `DELETE` for a cutoff age, evaluated
    /// against the database clock.
    fn inbox_purge_query(age: Duration) -> QueryBuilder<Self>;

    /// Claim up to `batch_size` outbox messages. This is the one genuinely
    /// divergent operation: Postgres uses a CTE with `FOR UPDATE SKIP LOCKED`;
    /// SQLite (no row locks) scans candidates and claims them with per-id
    /// conditional updates.
    fn claim_outbox<'a>(
        pool: &'a Pool<Self>,
        request: ClaimOutboxMessages,
    ) -> impl Future<Output = Result<Vec<OutboxMessage>, RepositoryError>> + Send + 'a;
}

/// SQL-backed async repository generic over the SQLx backend.
///
/// Use through the public aliases: [`PostgresRepository`](crate::PostgresRepository)
/// and [`SqliteRepository`](crate::SqliteRepository).
pub struct SqlxRepository<DB: sqlx::Database> {
    pool: Pool<DB>,
    read_model_schemas: Arc<RwLock<TableSchemaRegistry>>,
}

impl<DB: sqlx::Database> Clone for SqlxRepository<DB> {
    fn clone(&self) -> Self {
        Self {
            pool: self.pool.clone(),
            read_model_schemas: Arc::clone(&self.read_model_schemas),
        }
    }
}

/// SQL-backed outbox table store generic over the SQLx backend.
///
/// Use through the public aliases: [`PostgresOutboxStore`](crate::PostgresOutboxStore)
/// and [`SqliteOutboxStore`](crate::SqliteOutboxStore).
pub struct SqlxOutboxStore<DB: sqlx::Database> {
    pool: Pool<DB>,
}

impl<DB: sqlx::Database> Clone for SqlxOutboxStore<DB> {
    fn clone(&self) -> Self {
        Self {
            pool: self.pool.clone(),
        }
    }
}

impl<DB> SqlxRepository<DB>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    for<'c> &'c Pool<DB>: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
{
    /// Create a repository from an existing migrated pool.
    pub fn new(pool: Pool<DB>) -> Self {
        Self {
            pool,
            read_model_schemas: Arc::new(RwLock::new(TableSchemaRegistry::new())),
        }
    }

    /// Open a pool without applying migrations.
    pub async fn connect(database_url: &str) -> Result<Self, RepositoryError> {
        let pool = PoolOptions::<DB>::new()
            .max_connections(DB::default_pool_size(database_url))
            .connect(database_url)
            .await
            .map_err(|err| repository_storage_error::<DB>("connect", err))?;
        Ok(Self::new(pool))
    }

    /// Open a pool and apply this backend's explicit migrations.
    pub async fn connect_and_migrate(database_url: &str) -> Result<Self, RepositoryError>
    where
        DB::Connection: Migrate,
    {
        let repo = Self::connect(database_url).await?;
        repo.migrate().await?;
        Ok(repo)
    }

    /// Apply this backend's migrations to the repository's pool.
    pub async fn migrate(&self) -> Result<(), RepositoryError>
    where
        DB::Connection: Migrate,
    {
        Self::migrate_pool(&self.pool).await
    }

    /// Apply this backend's migrations to an existing pool. Applied versions
    /// are recorded in the `_sqlx_migrations` ledger, so re-running is a no-op
    /// and an edited already-applied migration fails its checksum comparison
    /// instead of silently diverging.
    pub async fn migrate_pool(pool: &Pool<DB>) -> Result<(), RepositoryError>
    where
        DB::Connection: Migrate,
    {
        DB::migrator()
            .run(pool)
            .await
            .map_err(|err| RepositoryError::Storage {
                operation: format!("{} migrate", DB::BACKEND),
                retryable: false,
                source: Some(Box::new(err)),
            })
    }

    /// Access the underlying SQLx pool for application-specific setup or tests.
    pub fn pool(&self) -> &Pool<DB> {
        &self.pool
    }

    /// SQL artifact adapter for registered table/read-model schemas.
    pub fn table_schema_adapter(&self) -> TableSqlSchemaAdapter {
        table_schema_adapter::<DB>()
    }

    /// Generate SQL statements for registered table/read-model schemas.
    pub fn generate_table_migration_artifacts(
        &self,
        registry: &TableSchemaRegistry,
    ) -> Result<Vec<TableMigrationArtifact>, TableStoreError> {
        generate_table_migration_artifacts(registry, DB::TABLE_DIALECT)
    }

    /// Explicit dev/test bootstrap for registered table/read-model schemas.
    pub async fn bootstrap_table_schema_for_dev(
        &self,
        registry: &TableSchemaRegistry,
    ) -> Result<TableSchemaBootstrap, TableStoreError> {
        bootstrap_table_schema(&self.pool, registry).await?;
        remember_read_model_schemas(&self.read_model_schemas, registry)?;
        Ok(table_schema_bootstrap_result(registry))
    }

    /// Access an outbox-store handle backed by this repository's pool.
    pub fn outbox_store(&self) -> SqlxOutboxStore<DB> {
        SqlxOutboxStore {
            pool: self.pool.clone(),
        }
    }
}

impl<DB> SqlxOutboxStore<DB>
where
    DB: SqlxRepoBackend,
    for<'c> &'c Pool<DB>: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
{
    pub fn new(pool: Pool<DB>) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &Pool<DB> {
        &self.pool
    }

    /// SQL artifact adapter for registered table/read-model schemas.
    pub fn table_schema_adapter(&self) -> TableSqlSchemaAdapter {
        table_schema_adapter::<DB>()
    }

    /// Generate SQL statements for registered table/read-model schemas.
    pub fn generate_table_migration_artifacts(
        &self,
        registry: &TableSchemaRegistry,
    ) -> Result<Vec<TableMigrationArtifact>, TableStoreError> {
        generate_table_migration_artifacts(registry, DB::TABLE_DIALECT)
    }

    /// Explicit dev/test bootstrap for registered table/read-model schemas.
    pub async fn bootstrap_table_schema_for_dev(
        &self,
        registry: &TableSchemaRegistry,
    ) -> Result<TableSchemaBootstrap, TableStoreError> {
        bootstrap_table_schema(&self.pool, registry).await?;
        Ok(table_schema_bootstrap_result(registry))
    }
}

fn table_schema_adapter<DB: SqlxRepoBackend>() -> TableSqlSchemaAdapter {
    match DB::TABLE_DIALECT {
        TableSqlDialect::Postgres => TableSqlSchemaAdapter::postgres(),
        TableSqlDialect::Sqlite => TableSqlSchemaAdapter::sqlite(),
    }
}

async fn bootstrap_table_schema<DB>(
    pool: &Pool<DB>,
    registry: &TableSchemaRegistry,
) -> Result<(), TableStoreError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c Pool<DB>: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
{
    for statement in table_schema_statements(registry, DB::TABLE_DIALECT)? {
        sqlx::query(audited_table_schema_sql(statement))
            .execute(pool)
            .await
            .map_err(|err| {
                TableStoreError::Storage(format!(
                    "{} bootstrap table schema failed: {err}",
                    DB::BACKEND
                ))
            })?;
    }
    Ok(())
}

impl<DB> GetStream for SqlxRepository<DB>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    for<'c> &'c Pool<DB>: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> f64: Encode<'q, DB> + Type<DB>,
    for<'q> String: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    fn get_stream<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<Option<Entity>, RepositoryError>> + Send + 'a {
        async move {
            let mut builder = QueryBuilder::<DB>::new("SELECT ");
            builder.push(DB::EVENT_SELECT);
            builder.push(" FROM aggregate_events WHERE aggregate_type = ");
            builder.push_bind(identity.aggregate_type());
            builder.push(" AND aggregate_id = ");
            builder.push_bind(identity.aggregate_id());
            builder.push(" ORDER BY sequence ASC");
            let rows = builder
                .build()
                .fetch_all(&self.pool)
                .await
                .map_err(|err| repository_storage_error::<DB>("load stream", err))?;

            if rows.is_empty() {
                return Ok(None);
            }

            let mut events = Vec::with_capacity(rows.len());
            for row in rows {
                events.push(event_from_row::<DB>(row)?);
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

            let mut entities = Vec::with_capacity(identities.len());
            for (aggregate_type, aggregate_ids) in ids_by_type(identities) {
                // Ordering by aggregate_id then sequence lets us slice the flat
                // result into per-aggregate entities in one pass. Callers of
                // `get_all` accept storage-order results.
                let mut builder = QueryBuilder::<DB>::new("SELECT aggregate_id, ");
                builder.push(DB::EVENT_SELECT);
                builder.push(" FROM aggregate_events WHERE aggregate_type = ");
                builder.push_bind(aggregate_type);
                builder.push(" AND ");
                DB::push_id_filter(&mut builder, &aggregate_ids);
                builder.push(" ORDER BY aggregate_id ASC, sequence ASC");

                let rows = builder
                    .build()
                    .fetch_all(&self.pool)
                    .await
                    .map_err(|err| repository_storage_error::<DB>("load streams", err))?;

                let mut current_id: Option<String> = None;
                let mut current_events: Vec<EventRecord> = Vec::new();
                for row in rows {
                    let row_id: String = row.try_get("aggregate_id").map_err(|err| {
                        repository_storage_error::<DB>("decode aggregate id row", err)
                    })?;
                    let event = event_from_row::<DB>(row)?;
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
            let after = repository_i64_from_u64(
                DB::BACKEND,
                after_version,
                "snapshot tail lower bound",
                DB::INTEGER_STORAGE,
            )?;
            let mut builder = QueryBuilder::<DB>::new("SELECT ");
            builder.push(DB::EVENT_SELECT);
            builder.push(" FROM aggregate_events WHERE aggregate_type = ");
            builder.push_bind(identity.aggregate_type());
            builder.push(" AND aggregate_id = ");
            builder.push_bind(identity.aggregate_id());
            builder.push(" AND sequence > ");
            builder.push_bind(after);
            builder.push(" ORDER BY sequence ASC");
            let rows = builder
                .build()
                .fetch_all(&self.pool)
                .await
                .map_err(|err| repository_storage_error::<DB>("load stream tail", err))?;

            // An empty tail is ambiguous from this query alone (no rows could
            // mean "snapshot is current" or "stream does not exist"). The
            // snapshot hydrate path only calls this after confirming a snapshot
            // exists for the identity, so an empty tail means the snapshot is
            // current. Return an entity at exactly `after_version`.
            let mut events = Vec::with_capacity(rows.len());
            for row in rows {
                events.push(event_from_row::<DB>(row)?);
            }

            let mut entity = Entity::new();
            entity.set_id(identity.aggregate_id());
            entity.load_tail_from_history(events, after_version);
            Ok(Some(entity))
        }
    }
}

impl<DB> TransactionalCommit for SqlxRepository<DB>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    for<'c> &'c Pool<DB>: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> f64: Encode<'q, DB> + Type<DB>,
    for<'q> String: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
    for<'q> Option<i64>: Encode<'q, DB> + Type<DB>,
    for<'q> Option<&'q str>: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    fn commit_batch<'a>(
        &'a self,
        batch: CommitBatch<'a>,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        async move {
            let prepared = validate_commit_batch(&batch)?;

            for plan in &batch.read_model_plans {
                validate_sql_write_plan(plan)?;
            }

            let mut tx =
                self.pool.begin().await.map_err(|err| {
                    repository_storage_error::<DB>("begin commit transaction", err)
                })?;

            // One grouped round trip for the whole batch's optimistic
            // concurrency pre-check instead of a MAX(sequence) query per stream.
            let versions = stream_versions_in_tx(&mut tx, &prepared).await?;
            for append in &prepared {
                let actual = versions
                    .get(&append.identity.storage_key())
                    .copied()
                    .unwrap_or(0);
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
                .map_err(|err| repository_storage_error::<DB>("commit transaction", err))?;

            for stream in batch.streams {
                stream.entity.mark_committed();
            }

            Ok(())
        }
    }
}

impl<DB> InboxStore for SqlxRepository<DB>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    for<'c> &'c Pool<DB>: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    fn inbox_contains<'a>(
        &'a self,
        consumer: &'a str,
        message_id: &'a str,
    ) -> impl Future<Output = Result<bool, RepositoryError>> + Send + 'a {
        async move {
            let mut builder =
                QueryBuilder::<DB>::new("SELECT 1 FROM consumer_inbox WHERE consumer = ");
            builder.push_bind(consumer);
            builder.push(" AND message_id = ");
            builder.push_bind(message_id);
            builder.push(" LIMIT 1");
            let row = builder
                .build()
                .fetch_optional(&self.pool)
                .await
                .map_err(|err| repository_storage_error::<DB>("query consumer inbox", err))?;
            Ok(row.is_some())
        }
    }

    fn purge_inbox_older_than(
        &self,
        age: std::time::Duration,
    ) -> impl Future<Output = Result<u64, RepositoryError>> + Send {
        async move {
            // Compare against the database clock to avoid client/server skew;
            // the backend renders the cutoff expression.
            let mut builder = DB::inbox_purge_query(age);
            let result = builder
                .build()
                .execute(&self.pool)
                .await
                .map_err(|err| repository_storage_error::<DB>("purge consumer inbox", err))?;
            Ok(DB::rows_affected(&result))
        }
    }
}

impl<DB> ReadModelWritePlanStore for SqlxRepository<DB>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
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

impl<DB> RelationalReadModelQueryStore for SqlxRepository<DB>
where
    DB: SqlxRepoBackend,
    for<'c> &'c Pool<DB>: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
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

impl<DB> SnapshotStore for SqlxRepository<DB>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    for<'c> &'c Pool<DB>: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    fn get_snapshot<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<Option<SnapshotRecord>, RepositoryError>> + Send + 'a {
        async move {
            let mut builder = QueryBuilder::<DB>::new("SELECT ");
            builder.push(DB::SNAPSHOT_SELECT);
            builder.push(" FROM aggregate_snapshots WHERE aggregate_type = ");
            builder.push_bind(identity.aggregate_type());
            builder.push(" AND aggregate_id = ");
            builder.push_bind(identity.aggregate_id());
            let row = builder
                .build()
                .fetch_optional(&self.pool)
                .await
                .map_err(|err| repository_storage_error::<DB>("load snapshot", err))?;

            let Some(row) = row else {
                return Ok(None);
            };

            Ok(Some(snapshot_from_row::<DB>(row)?))
        }
    }

    fn get_snapshots<'a>(
        &'a self,
        identities: &'a [StreamIdentity],
    ) -> impl Future<Output = Result<Vec<SnapshotRecord>, RepositoryError>> + Send + 'a {
        async move {
            if identities.is_empty() {
                return Ok(Vec::new());
            }

            let mut records = Vec::with_capacity(identities.len());
            for (aggregate_type, aggregate_ids) in ids_by_type(identities) {
                let mut builder = QueryBuilder::<DB>::new("SELECT ");
                builder.push(DB::SNAPSHOT_SELECT);
                builder.push(" FROM aggregate_snapshots WHERE aggregate_type = ");
                builder.push_bind(aggregate_type);
                builder.push(" AND ");
                DB::push_id_filter(&mut builder, &aggregate_ids);
                let rows = builder
                    .build()
                    .fetch_all(&self.pool)
                    .await
                    .map_err(|err| repository_storage_error::<DB>("load snapshots", err))?;
                for row in rows {
                    records.push(snapshot_from_row::<DB>(row)?);
                }
            }
            Ok(records)
        }
    }

    fn save_snapshot<'a>(
        &'a self,
        identity: &'a StreamIdentity,
        record: SnapshotRecord,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        async move {
            let mut tx =
                self.pool.begin().await.map_err(|err| {
                    repository_storage_error::<DB>("begin snapshot transaction", err)
                })?;
            save_snapshot_in_tx(&mut tx, identity, record).await?;
            tx.commit().await.map_err(|err| {
                repository_storage_error::<DB>("commit snapshot transaction", err)
            })?;
            Ok(())
        }
    }

    fn delete_snapshot<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<bool, RepositoryError>> + Send + 'a {
        async move {
            let mut builder =
                QueryBuilder::<DB>::new("DELETE FROM aggregate_snapshots WHERE aggregate_type = ");
            builder.push_bind(identity.aggregate_type());
            builder.push(" AND aggregate_id = ");
            builder.push_bind(identity.aggregate_id());
            let result = builder
                .build()
                .execute(&self.pool)
                .await
                .map_err(|err| repository_storage_error::<DB>("delete snapshot", err))?;

            Ok(DB::rows_affected(&result) > 0)
        }
    }
}

/// One claimed-message lifecycle transition (the `UPDATE` shape is shared; only
/// the assignments differ).
enum OutboxTransition<'a> {
    Complete,
    Release { error: &'a str },
    Fail { error: &'a str },
}

impl OutboxTransition<'_> {
    fn target_status(&self) -> OutboxMessageStatus {
        match self {
            OutboxTransition::Complete => OutboxMessageStatus::Published,
            OutboxTransition::Release { .. } => OutboxMessageStatus::Pending,
            OutboxTransition::Fail { .. } => OutboxMessageStatus::Failed,
        }
    }

    fn operation(&self) -> &'static str {
        match self {
            OutboxTransition::Complete => "complete outbox message",
            OutboxTransition::Release { .. } => "release outbox message",
            OutboxTransition::Fail { .. } => "fail outbox message",
        }
    }
}

impl<DB> OutboxStore for SqlxOutboxStore<DB>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    for<'c> &'c Pool<DB>: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> f64: Encode<'q, DB> + Type<DB>,
    for<'q> String: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
    for<'q> Option<i64>: Encode<'q, DB> + Type<DB>,
    for<'q> Option<&'q str>: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    fn messages_by_status(
        &self,
        status: OutboxMessageStatus,
        limit: usize,
    ) -> impl Future<Output = Result<Vec<OutboxMessage>, RepositoryError>> + Send + '_ {
        async move {
            let mut builder = QueryBuilder::<DB>::new("SELECT ");
            builder.push(DB::OUTBOX_SELECT);
            builder.push(" FROM outbox_messages WHERE status = ");
            builder.push_bind(status.as_str());
            builder.push(" ORDER BY ");
            builder.push(DB::ORDER_BY_CREATED_AT);
            builder.push(" ASC, message_id ASC LIMIT ");
            // usize::MAX means "no practical bound"; clamp to what the column
            // type can carry.
            builder.push_bind(i64::try_from(limit).unwrap_or(i64::MAX));
            let rows = builder.build().fetch_all(&self.pool).await.map_err(|err| {
                repository_storage_error::<DB>("load outbox messages by status", err)
            })?;

            rows.into_iter()
                .map(outbox_message_from_row::<DB>)
                .collect()
        }
    }

    fn claim<'a>(
        &'a self,
        request: ClaimOutboxMessages,
    ) -> impl Future<Output = Result<Vec<OutboxMessage>, RepositoryError>> + Send + 'a {
        DB::claim_outbox(&self.pool, request)
    }

    fn complete<'a>(
        &'a self,
        claim: &'a OutboxClaimRef,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        transition_claimed_outbox_message(&self.pool, claim, OutboxTransition::Complete)
    }

    fn release<'a>(
        &'a self,
        claim: &'a OutboxClaimRef,
        error: &'a str,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        transition_claimed_outbox_message(&self.pool, claim, OutboxTransition::Release { error })
    }

    fn fail<'a>(
        &'a self,
        claim: &'a OutboxClaimRef,
        error: &'a str,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        transition_claimed_outbox_message(&self.pool, claim, OutboxTransition::Fail { error })
    }
}

/// Apply one claimed-message lifecycle transition (complete / release / fail).
///
/// The conditional `UPDATE` only applies while the caller still holds the
/// active claim (`status`, `claimed_by`, unexpired `claimed_until`, and
/// matching `attempts`); when no row is updated, the message is re-read to
/// produce the precise claim error.
async fn transition_claimed_outbox_message<'a, DB>(
    pool: &'a Pool<DB>,
    claim: &'a OutboxClaimRef,
    transition: OutboxTransition<'a>,
) -> Result<(), RepositoryError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c Pool<DB>: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> f64: Encode<'q, DB> + Type<DB>,
    for<'q> String: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> Option<i64>: Encode<'q, DB> + Type<DB>,
    for<'q> Option<&'q str>: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let now = SystemTime::now();
    let now_epoch = system_time_epoch_secs::<DB>(now)?;
    let now_value = DB::timestamp_value(now)?;

    let mut builder = QueryBuilder::<DB>::new("UPDATE outbox_messages SET status = ");
    builder.push_bind(transition.target_status().as_str());
    builder.push(", claimed_by = NULL, claimed_until = NULL, ");
    match &transition {
        OutboxTransition::Complete => {
            builder.push("published_at = ");
            DB::push_timestamp_assign(&mut builder, &now_value);
        }
        OutboxTransition::Release { error } => {
            builder.push("next_available_at = ");
            DB::push_timestamp_assign(&mut builder, &now_value);
            builder.push(", last_error = ");
            builder.push_bind(empty_string_as_none(error));
        }
        OutboxTransition::Fail { error } => {
            builder.push("last_error = ");
            builder.push_bind(empty_string_as_none(error));
            builder.push(", failed_at = ");
            DB::push_timestamp_assign(&mut builder, &now_value);
        }
    }
    builder.push(", updated_at = ");
    builder.push(DB::NOW);
    builder.push(" WHERE message_id = ");
    builder.push_bind(claim.message_id.as_str());
    builder.push(" AND status = ");
    builder.push_bind(OutboxMessageStatus::InFlight.as_str());
    builder.push(" AND claimed_by = ");
    builder.push_bind(claim.worker_id.as_str());
    builder.push(" AND claimed_until IS NOT NULL AND ");
    DB::push_timestamp_cmp(&mut builder, "claimed_until", ">", now_epoch);
    builder.push(" AND attempts = ");
    builder.push_bind(repository_i64_from_u64(
        DB::BACKEND,
        u64::from(claim.attempt),
        "outbox claim attempt",
        DB::INTEGER_STORAGE,
    )?);

    let result = builder
        .build()
        .execute(pool)
        .await
        .map_err(|err| repository_storage_error::<DB>(transition.operation(), err))?;

    ensure_outbox_update_applied(
        pool,
        DB::rows_affected(&result),
        &claim.message_id,
        |message| ensure_active_claim(message, Some(claim), now),
    )
    .await
}

/// Load an outbox message by id through any executor (pool or transaction).
pub(crate) async fn outbox_message_by_id<'e, DB, E>(
    executor: E,
    message_id: &str,
) -> Result<Option<OutboxMessage>, RepositoryError>
where
    DB: SqlxRepoBackend,
    E: Executor<'e, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let mut builder = QueryBuilder::<DB>::new("SELECT ");
    builder.push(DB::OUTBOX_SELECT);
    builder.push(" FROM outbox_messages WHERE message_id = ");
    builder.push_bind(message_id);
    let row = builder
        .build()
        .fetch_optional(executor)
        .await
        .map_err(|err| repository_storage_error::<DB>("load outbox message", err))?;
    row.map(outbox_message_from_row::<DB>).transpose()
}

pub(crate) async fn ensure_outbox_update_applied<DB>(
    pool: &Pool<DB>,
    rows_affected: u64,
    message_id: &str,
    validate: impl FnOnce(&OutboxMessage) -> Result<(), RepositoryError>,
) -> Result<(), RepositoryError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c Pool<DB>: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    if rows_affected > 0 {
        return Ok(());
    }

    let message = outbox_message_by_id(pool, message_id)
        .await?
        .ok_or_else(|| RepositoryError::NotFound {
            id: message_id.to_string(),
        })?;
    validate(&message)
}

/// Record a consumer inbox receipt in the commit transaction. The
/// `(consumer, message_id)` primary key is the dedupe gate: a unique violation
/// means the message was already processed, so the whole batch rolls back and
/// the effects are not double-applied. `processed_at` defaults server-side.
async fn insert_inbox_receipt_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    receipt: &InboxReceipt,
) -> Result<(), RepositoryError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
{
    receipt.validate()?;
    let mut builder =
        QueryBuilder::<DB>::new("INSERT INTO consumer_inbox (consumer, message_id) VALUES (");
    builder.push_bind(receipt.consumer.as_str());
    builder.push(", ");
    builder.push_bind(receipt.message_id.as_str());
    builder.push(")");
    let result = builder.build().execute(&mut **tx).await;
    match result {
        Ok(_) => Ok(()),
        Err(err) if DB::is_unique_violation(&err) => Err(RepositoryError::DuplicateInboxReceipt {
            consumer: receipt.consumer.clone(),
            message_id: receipt.message_id.clone(),
        }),
        Err(err) => Err(repository_storage_error::<DB>(
            "insert consumer inbox receipt",
            err,
        )),
    }
}

/// One `aggregate_events` row with pre-validated bind values, built before the
/// query so any conversion error surfaces before we touch the database. The
/// stream identity and expected version ride along for conflict recovery.
struct EventRow<'a, DB: SqlxRepoBackend> {
    identity: &'a StreamIdentity,
    expected_version: u64,
    sequence: i64,
    event_name: &'a str,
    event_version: i64,
    payload: &'a [u8],
    payload_codec: &'a str,
    payload_codec_version: i64,
    metadata: String,
    recorded_at: DB::TimestampValue,
}

/// Insert every event across all prepared appends with multi-row INSERTs,
/// chunked to respect the backend's bound-parameter limit (Postgres is
/// effectively unlimited, so its chunking collapses to one statement).
///
/// Conflict detection is unchanged from the per-row path: the `(aggregate_type,
/// aggregate_id, sequence)` primary key is the contiguity gate, and a unique
/// violation still surfaces as `ConcurrentWrite`. Recovery re-reads stream
/// versions in-tx or over the pool depending on
/// [`SqlxRepoBackend::CONFLICT_REREAD_IN_TX`].
async fn insert_events_in_tx<DB>(
    pool: &Pool<DB>,
    tx: &mut Transaction<'_, DB>,
    prepared: &[PreparedEventAppend<'_>],
) -> Result<(), RepositoryError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    for<'c> &'c Pool<DB>: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let mut rows = Vec::new();
    for append in prepared {
        for event in append.events {
            rows.push(EventRow::<DB> {
                identity: &append.identity,
                expected_version: append.expected_version,
                sequence: repository_i64_from_u64(
                    DB::BACKEND,
                    event.sequence,
                    "sequence",
                    DB::INTEGER_STORAGE,
                )?,
                event_name: &event.event_name,
                event_version: repository_i64_from_u64(
                    DB::BACKEND,
                    event.event_version,
                    "event_version",
                    DB::INTEGER_STORAGE,
                )?,
                payload: &event.payload,
                payload_codec: &event.payload_codec,
                payload_codec_version: i64::from(event.payload_codec_version),
                metadata: serialize_event_metadata(&event.metadata)?,
                recorded_at: DB::timestamp_value(event.timestamp)?,
            });
        }
    }

    for chunk in rows.chunks(DB::MAX_BIND_PARAMS / EVENT_BIND_COLUMNS) {
        let mut builder = QueryBuilder::<DB>::new(
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
                .push_bind(event.payload_codec_version);
            DB::push_metadata(&mut row, event.metadata.as_str());
            DB::push_timestamp(&mut row, &event.recorded_at);
        });

        let result = builder.build().execute(&mut **tx).await;
        match result {
            Ok(_) => {}
            Err(err) if DB::is_unique_violation(&err) => {
                return Err(if DB::CONFLICT_REREAD_IN_TX {
                    // The transaction survives the constraint error: re-read in
                    // the same tx, scoped to this chunk (earlier chunks were
                    // already inserted in this tx and would skew the versions
                    // of their streams).
                    let mut seen = std::collections::HashSet::new();
                    let candidates: Vec<_> = chunk
                        .iter()
                        .filter(|event| seen.insert(event.identity.storage_key()))
                        .map(|event| (event.identity, event.expected_version))
                        .collect();
                    concurrent_write_from_conflict(&mut **tx, &candidates).await
                } else {
                    // The failed statement aborted the transaction: re-read the
                    // conflicting streams' actual versions on a separate
                    // connection, across the whole batch.
                    let candidates: Vec<_> = prepared
                        .iter()
                        .map(|append| (&append.identity, append.expected_version))
                        .collect();
                    match pool.acquire().await {
                        Ok(mut conn) => {
                            concurrent_write_from_conflict(&mut conn, &candidates).await
                        }
                        Err(err) => repository_storage_error::<DB>(
                            "acquire conflict re-read connection",
                            err,
                        ),
                    }
                });
            }
            Err(err) => return Err(repository_storage_error::<DB>("insert events", err)),
        }
    }

    Ok(())
}

/// After an event-insert unique violation, find the candidate stream whose
/// actual version no longer matches its expected version and report it as
/// `ConcurrentWrite`. Falls back to the first candidate if a concurrent
/// writer's effect cannot be pinned down (the violation still indicates a
/// conflicting write). Candidates must be non-empty and deduplicated.
async fn concurrent_write_from_conflict<DB>(
    conn: &mut DB::Connection,
    candidates: &[(&StreamIdentity, u64)],
) -> RepositoryError
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    for &(identity, expected) in candidates {
        match stream_version(&mut *conn, identity).await {
            Ok(actual) if actual != expected => {
                return RepositoryError::ConcurrentWrite {
                    id: identity.to_string(),
                    expected,
                    actual,
                };
            }
            Ok(_) => {}
            Err(err) => return err,
        }
    }

    let (identity, expected) = candidates[0];
    match stream_version(&mut *conn, identity).await {
        Ok(actual) => RepositoryError::ConcurrentWrite {
            id: identity.to_string(),
            expected,
            actual,
        },
        Err(err) => err,
    }
}

/// Current committed version (`MAX(sequence)`, 0 for a missing stream) through
/// any executor (pool or transaction).
async fn stream_version<'e, DB, E>(
    executor: E,
    identity: &StreamIdentity,
) -> Result<u64, RepositoryError>
where
    DB: SqlxRepoBackend,
    E: Executor<'e, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let mut builder = QueryBuilder::<DB>::new(
        "SELECT MAX(sequence) AS version FROM aggregate_events WHERE aggregate_type = ",
    );
    builder.push_bind(identity.aggregate_type());
    builder.push(" AND aggregate_id = ");
    builder.push_bind(identity.aggregate_id());
    let row = builder
        .build()
        .fetch_one(executor)
        .await
        .map_err(|err| repository_storage_error::<DB>("load stream version", err))?;

    let version: Option<i64> = row
        .try_get("version")
        .map_err(|err| repository_storage_error::<DB>("decode stream version row", err))?;
    version
        .map(|value| repository_u64_from_i64(DB::BACKEND, value, "sequence"))
        .unwrap_or(Ok(0))
}

/// Current committed versions for every stream in the batch, in one grouped
/// query (`MAX(sequence)` per stream; missing streams simply have no row and
/// default to 0 at the call site). Chunked so a very large batch stays under
/// the backend's bound-parameter limit (two binds per stream).
async fn stream_versions_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    prepared: &[PreparedEventAppend<'_>],
) -> Result<HashMap<String, u64>, RepositoryError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let mut versions = HashMap::with_capacity(prepared.len());
    if prepared.is_empty() {
        return Ok(versions);
    }

    for chunk in prepared.chunks(DB::MAX_BIND_PARAMS / 2) {
        let mut builder = QueryBuilder::<DB>::new(
            "SELECT aggregate_type, aggregate_id, MAX(sequence) AS version \
             FROM aggregate_events WHERE ",
        );
        let mut first = true;
        for append in chunk {
            if !first {
                builder.push(" OR ");
            }
            first = false;
            builder.push("(aggregate_type = ");
            builder.push_bind(append.identity.aggregate_type());
            builder.push(" AND aggregate_id = ");
            builder.push_bind(append.identity.aggregate_id());
            builder.push(")");
        }
        builder.push(" GROUP BY aggregate_type, aggregate_id");

        let rows = builder
            .build()
            .fetch_all(&mut **tx)
            .await
            .map_err(|err| repository_storage_error::<DB>("load stream versions", err))?;

        for row in rows {
            let aggregate_type: String = row.try_get("aggregate_type").map_err(|err| {
                repository_storage_error::<DB>("decode stream version aggregate type row", err)
            })?;
            let aggregate_id: String = row.try_get("aggregate_id").map_err(|err| {
                repository_storage_error::<DB>("decode stream version aggregate id row", err)
            })?;
            let version: i64 = row
                .try_get("version")
                .map_err(|err| repository_storage_error::<DB>("decode stream version row", err))?;
            versions.insert(
                StreamIdentity::new(&aggregate_type, &aggregate_id)?.storage_key(),
                repository_u64_from_i64(DB::BACKEND, version, "sequence")?,
            );
        }
    }

    Ok(versions)
}

/// One `outbox_messages` row with pre-validated bind values.
struct OutboxRow<'a, DB: SqlxRepoBackend> {
    message_id: &'a str,
    event_type: &'a str,
    payload: &'a [u8],
    payload_codec: &'a str,
    payload_codec_version: i64,
    destination: Option<&'a str>,
    metadata: String,
    status: &'a str,
    created_at: DB::TimestampValue,
    worker_id: Option<&'a str>,
    leased_until: Option<DB::TimestampValue>,
    attempts: i64,
    last_error: Option<&'a str>,
    source_aggregate_type: Option<&'a str>,
    source_aggregate_id: Option<&'a str>,
    source_sequence: Option<i64>,
    correlation_id: Option<&'a str>,
    causation_id: Option<&'a str>,
}

/// Insert every outbox message with multi-row INSERTs (chunked to respect the
/// backend's bound-parameter limit). A unique violation on `message_id` still
/// maps to `DuplicateOutboxMessageInBatch`.
async fn insert_outbox_messages_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    messages: &[OutboxMessage],
) -> Result<(), RepositoryError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB>,
    for<'q> Option<i64>: Encode<'q, DB> + Type<DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> Option<&'q str>: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
{
    if messages.is_empty() {
        return Ok(());
    }

    let mut rows = Vec::with_capacity(messages.len());
    for message in messages {
        rows.push(OutboxRow::<DB> {
            message_id: message.id(),
            event_type: &message.event_type,
            payload: &message.payload,
            payload_codec: &message.payload_codec,
            payload_codec_version: i64::from(message.payload_codec_version),
            destination: message.destination.as_deref(),
            metadata: serialize_event_metadata(&message.metadata)?,
            status: message.status.as_str(),
            created_at: DB::timestamp_value(message.created_at)?,
            worker_id: message.worker_id.as_deref(),
            leased_until: message.leased_until.map(DB::timestamp_value).transpose()?,
            attempts: i64::from(message.attempts),
            last_error: message.last_error.as_deref(),
            source_aggregate_type: message.source_aggregate_type.as_deref(),
            source_aggregate_id: message.source_aggregate_id.as_deref(),
            source_sequence: message
                .source_sequence
                .map(|value| {
                    repository_i64_from_u64(
                        DB::BACKEND,
                        value,
                        "outbox source sequence",
                        DB::INTEGER_STORAGE,
                    )
                })
                .transpose()?,
            correlation_id: message.correlation_id(),
            causation_id: message.causation_id(),
        });
    }

    for chunk in rows.chunks(DB::MAX_BIND_PARAMS / OUTBOX_BIND_COLUMNS) {
        let mut builder = QueryBuilder::<DB>::new(
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
                .push_bind(message.destination);
            DB::push_metadata(&mut row, message.metadata.as_str());
            row.push_bind(message.status);
            // created_at and next_available_at share the same value.
            DB::push_timestamp(&mut row, &message.created_at);
            DB::push_timestamp(&mut row, &message.created_at);
            row.push_bind(message.worker_id);
            DB::push_optional_timestamp(&mut row, message.leased_until.as_ref());
            row.push_bind(message.attempts)
                .push_bind(message.last_error)
                .push_bind(message.source_aggregate_type)
                .push_bind(message.source_aggregate_id)
                .push_bind(message.source_sequence)
                .push_bind(message.correlation_id)
                .push_bind(message.causation_id);
        });

        let result = builder.build().execute(&mut **tx).await;
        if let Err(err) = result {
            if DB::is_unique_violation(&err) {
                // The batch was already deduped (validate_commit_batch), so a
                // violation means the id collides with a previously committed
                // row. Report the first id in the chunk, matching the per-row
                // path's contract.
                return Err(RepositoryError::DuplicateOutboxMessageInBatch {
                    id: chunk[0].message_id.to_string(),
                });
            }
            return Err(repository_storage_error::<DB>(
                "insert outbox messages",
                err,
            ));
        }
    }

    Ok(())
}

async fn save_snapshot_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    identity: &StreamIdentity,
    record: SnapshotRecord,
) -> Result<(), RepositoryError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
{
    validate_snapshot_identity(identity, &record)?;

    let metadata = serialize_event_metadata(&record.metadata)?;
    let recorded_at = DB::timestamp_value(record.recorded_at)?;
    let version = repository_i64_from_u64(
        DB::BACKEND,
        record.version,
        "snapshot version",
        DB::INTEGER_STORAGE,
    )?;
    let snapshot_version = repository_i64_from_u64(
        DB::BACKEND,
        record.snapshot_version,
        "snapshot payload version",
        DB::INTEGER_STORAGE,
    )?;

    let mut builder = QueryBuilder::<DB>::new(
        "INSERT INTO aggregate_snapshots (\
         aggregate_type, aggregate_id, version, snapshot_version, payload, \
         payload_codec, payload_codec_version, metadata, recorded_at) VALUES (",
    );
    {
        let mut row = builder.separated(", ");
        row.push_bind(identity.aggregate_type())
            .push_bind(identity.aggregate_id())
            .push_bind(version)
            .push_bind(snapshot_version)
            .push_bind(record.payload.as_slice())
            .push_bind(record.payload_codec.as_str())
            .push_bind(i64::from(record.payload_codec_version));
        DB::push_metadata(&mut row, metadata.as_str());
        DB::push_timestamp(&mut row, &recorded_at);
    }
    builder.push(
        ") ON CONFLICT(aggregate_type, aggregate_id) DO UPDATE SET \
         version = excluded.version, \
         snapshot_version = excluded.snapshot_version, \
         payload = excluded.payload, \
         payload_codec = excluded.payload_codec, \
         payload_codec_version = excluded.payload_codec_version, \
         metadata = excluded.metadata, \
         recorded_at = excluded.recorded_at, \
         updated_at = ",
    );
    builder.push(DB::NOW);

    builder
        .build()
        .execute(&mut **tx)
        .await
        .map_err(|err| repository_storage_error::<DB>("save snapshot", err))?;

    Ok(())
}

fn entity_from_events(aggregate_id: String, events: Vec<EventRecord>) -> Entity {
    let mut entity = Entity::new();
    entity.set_id(aggregate_id);
    entity.load_from_history(events);
    entity
}

pub(crate) fn event_from_row<DB>(row: DB::Row) -> Result<EventRecord, RepositoryError>
where
    DB: SqlxRepoBackend,
    for<'q> i64: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let payload_codec: String = row
        .try_get("payload_codec")
        .map_err(|err| repository_storage_error::<DB>("decode payload codec row", err))?;
    // Nearly every row carries the crate's own codec constant; borrow it
    // instead of keeping a per-event allocation.
    let payload_codec = if payload_codec == BITCODE_PAYLOAD_CODEC {
        Cow::Borrowed(BITCODE_PAYLOAD_CODEC)
    } else {
        Cow::Owned(payload_codec)
    };
    let payload_codec_version = repository_u16_from_i64(
        DB::BACKEND,
        row.try_get("payload_codec_version").map_err(|err| {
            repository_storage_error::<DB>("decode payload codec version row", err)
        })?,
        "payload_codec_version",
    )?;
    let metadata_json: String = row
        .try_get("metadata")
        .map_err(|err| repository_storage_error::<DB>("decode metadata row", err))?;
    let metadata = deserialize_event_metadata(&metadata_json)?;
    let event = EventRecord {
        event_name: row
            .try_get("event_name")
            .map_err(|err| repository_storage_error::<DB>("decode event name row", err))?,
        payload_codec,
        payload_codec_version,
        payload: row
            .try_get("payload")
            .map_err(|err| repository_storage_error::<DB>("decode payload row", err))?,
        event_version: repository_u64_from_i64(
            DB::BACKEND,
            row.try_get("event_version")
                .map_err(|err| repository_storage_error::<DB>("decode event version row", err))?,
            "event_version",
        )?,
        sequence: repository_u64_from_i64(
            DB::BACKEND,
            row.try_get("sequence")
                .map_err(|err| repository_storage_error::<DB>("decode sequence row", err))?,
            "sequence",
        )?,
        timestamp: DB::decode_timestamp(&row, "recorded_at")?,
        metadata,
    };
    validate_supported_event_codec(&event)?;
    Ok(event)
}

fn snapshot_from_row<DB>(row: DB::Row) -> Result<SnapshotRecord, RepositoryError>
where
    DB: SqlxRepoBackend,
    for<'q> i64: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let metadata_json: String = row
        .try_get("metadata")
        .map_err(|err| repository_storage_error::<DB>("decode snapshot metadata row", err))?;
    Ok(SnapshotRecord {
        aggregate_type: row.try_get("aggregate_type").map_err(|err| {
            repository_storage_error::<DB>("decode snapshot aggregate type row", err)
        })?,
        aggregate_id: row.try_get("aggregate_id").map_err(|err| {
            repository_storage_error::<DB>("decode snapshot aggregate id row", err)
        })?,
        version: repository_u64_from_i64(
            DB::BACKEND,
            row.try_get("version").map_err(|err| {
                repository_storage_error::<DB>("decode snapshot version row", err)
            })?,
            "snapshot version",
        )?,
        snapshot_version: repository_u64_from_i64(
            DB::BACKEND,
            row.try_get("snapshot_version").map_err(|err| {
                repository_storage_error::<DB>("decode snapshot payload version row", err)
            })?,
            "snapshot payload version",
        )?,
        payload_codec: row.try_get("payload_codec").map_err(|err| {
            repository_storage_error::<DB>("decode snapshot payload codec row", err)
        })?,
        payload_codec_version: repository_u16_from_i64(
            DB::BACKEND,
            row.try_get("payload_codec_version").map_err(|err| {
                repository_storage_error::<DB>("decode snapshot payload codec version row", err)
            })?,
            "snapshot payload codec version",
        )?,
        payload: row
            .try_get("payload")
            .map_err(|err| repository_storage_error::<DB>("decode snapshot payload row", err))?,
        metadata: deserialize_event_metadata(&metadata_json)?,
        recorded_at: DB::decode_timestamp(&row, "recorded_at")?,
    })
}

pub(crate) fn outbox_message_from_row<DB>(row: DB::Row) -> Result<OutboxMessage, RepositoryError>
where
    DB: SqlxRepoBackend,
    for<'q> i64: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let status_text: String = row
        .try_get("status")
        .map_err(|err| repository_storage_error::<DB>("decode outbox status row", err))?;
    let status = status_text.parse::<OutboxMessageStatus>().map_err(|_| {
        RepositoryError::Model(format!(
            "{} outbox status `{status_text}` is invalid",
            DB::BACKEND
        ))
    })?;
    let metadata_json: String = row
        .try_get("metadata")
        .map_err(|err| repository_storage_error::<DB>("decode outbox metadata row", err))?;
    let attempts: i64 = row
        .try_get("attempts")
        .map_err(|err| repository_storage_error::<DB>("decode outbox attempts row", err))?;
    let source_sequence = row
        .try_get::<Option<i64>, _>("source_sequence")
        .map_err(|err| repository_storage_error::<DB>("decode outbox source sequence row", err))?
        .map(|value| repository_u64_from_i64(DB::BACKEND, value, "outbox source sequence"))
        .transpose()?;
    let mut metadata = deserialize_event_metadata(&metadata_json)?;
    if let Some(correlation_id) = row
        .try_get::<Option<String>, _>("correlation_id")
        .map_err(|err| repository_storage_error::<DB>("decode outbox correlation_id row", err))?
    {
        metadata.insert("correlation_id".into(), correlation_id);
    }
    if let Some(causation_id) = row
        .try_get::<Option<String>, _>("causation_id")
        .map_err(|err| repository_storage_error::<DB>("decode outbox causation_id row", err))?
    {
        metadata.insert("causation_id".into(), causation_id);
    }

    Ok(OutboxMessage {
        id: row
            .try_get("message_id")
            .map_err(|err| repository_storage_error::<DB>("decode outbox message id row", err))?,
        event_type: row
            .try_get("event_type")
            .map_err(|err| repository_storage_error::<DB>("decode outbox event type row", err))?,
        payload: row
            .try_get("payload")
            .map_err(|err| repository_storage_error::<DB>("decode outbox payload row", err))?,
        payload_codec: row.try_get("payload_codec").map_err(|err| {
            repository_storage_error::<DB>("decode outbox payload codec row", err)
        })?,
        payload_codec_version: repository_u16_from_i64(
            DB::BACKEND,
            row.try_get("payload_codec_version").map_err(|err| {
                repository_storage_error::<DB>("decode outbox payload codec version row", err)
            })?,
            "outbox payload codec version",
        )?,
        metadata,
        status,
        created_at: DB::decode_timestamp(&row, "created_at")?,
        worker_id: row
            .try_get("claimed_by")
            .map_err(|err| repository_storage_error::<DB>("decode outbox claimed_by row", err))?,
        leased_until: DB::decode_optional_timestamp(&row, "claimed_until")?,
        attempts: u32::try_from(attempts).map_err(|_| {
            RepositoryError::Model(format!(
                "{} outbox attempts value {attempts} is invalid",
                DB::BACKEND
            ))
        })?,
        last_error: row
            .try_get("last_error")
            .map_err(|err| repository_storage_error::<DB>("decode outbox last_error row", err))?,
        destination: row
            .try_get("destination")
            .map_err(|err| repository_storage_error::<DB>("decode outbox destination row", err))?,
        source_aggregate_type: row.try_get("source_aggregate_type").map_err(|err| {
            repository_storage_error::<DB>("decode outbox source aggregate type row", err)
        })?,
        source_aggregate_id: row.try_get("source_aggregate_id").map_err(|err| {
            repository_storage_error::<DB>("decode outbox source aggregate id row", err)
        })?,
        source_sequence,
    })
}

/// Convert a [`SystemTime`] to epoch seconds for database-side comparisons.
pub(crate) fn system_time_epoch_secs<DB: SqlxRepoBackend>(
    timestamp: SystemTime,
) -> Result<f64, RepositoryError> {
    let duration = timestamp.duration_since(UNIX_EPOCH).map_err(|err| {
        RepositoryError::Model(format!(
            "timestamp before UNIX epoch cannot be stored in {}: {err}",
            DB::BACKEND
        ))
    })?;
    Ok(duration.as_secs_f64())
}

pub(crate) fn repository_storage_error<DB: SqlxRepoBackend>(
    operation: &str,
    err: sqlx::Error,
) -> RepositoryError {
    crate::sqlx_repo::repository_storage_error(DB::BACKEND, operation, err)
}
