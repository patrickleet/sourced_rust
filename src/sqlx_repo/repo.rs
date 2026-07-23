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

use crate::command_ledger::{
    AttemptFence, AttemptToken, CanonicalInputHash, CausalCommitBatch, CausalGetStream,
    CausalRepositoryIdentity, CausalStorageIdentity, CausalTransactionalCommit, CausationId,
    CommandCompletion, CommandContractFingerprint, CommandId, CommandLedgerError, CommandLedgerKey,
    CommandLedgerRecord, CommandLedgerState, CommandLedgerStore, CommandLookup, CommandLookupScope,
    CommandReservation, PrincipalPartitionId, ReservationDecision, ReservationOutcome,
};
use crate::entity::{Entity, EventRecord, BITCODE_PAYLOAD_CODEC};
use crate::outbox::{OutboxMessage, OutboxMessageStatus};
use crate::outbox_worker::{
    ensure_active_claim, ClaimOutboxMessages, OutboxBacklogStats, OutboxClaimRef, OutboxStore,
};
use crate::projection_protocol::{ProjectionChangeRetention, SameTransactionProjectionBatch};
use crate::read_model::{ReadModelLoadGraph, ReadModelLoadRequest, ReadModelQueryCapabilities};
use crate::repository::{
    validate_commit_batch, validate_snapshot_identity, validate_supported_event_codec, CommitBatch,
    GetStream, InboxReceipt, InboxStore, PreparedEventAppend, ReadModelWritePlanStore,
    RelationalReadModelQueryStore, RepositoryError, SnapshotStore, SnapshotWrite, StreamIdentity,
    TransactionalCommit,
};
use crate::snapshot::SnapshotRecord;
use crate::sqlx_repo::projection_protocol::{
    apply_same_transaction_projection_in_tx, reject_causal_table_writes_in_tx,
    PROJECTION_CHANGE_NOTIFY_TABLE,
};
use crate::sqlx_repo::read_model::{
    apply_read_model_write_plan_in_tx, begin_read_model_tx, commit_read_model_tx,
    empty_string_as_none, load_read_model_graph, remember_read_model_schemas,
    sql_read_model_capabilities, validate_sql_write_plan, SqlxReadModelBackend,
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
    /// Command-ledger projection with timestamp columns normalized for
    /// [`decode_timestamp`](Self::decode_timestamp) and JSON surfaced as text.
    const COMMAND_LEDGER_SELECT: &'static str;
    /// Row-lock suffix for a reservation/status transaction.
    const COMMAND_LEDGER_LOCK_SUFFIX: &'static str;
    /// Row-lock suffix for a bounded compaction scan.
    const COMMAND_LEDGER_COMPACTION_LOCK_SUFFIX: &'static str;
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
    /// `SELECT` expression for `MIN(created_at)` surfaced as
    /// `oldest_created_at` in this backend's timestamp representation.
    const OUTBOX_OLDEST_CREATED_AT_SELECT: &'static str;
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

    /// Push a non-transaction-start database clock expression. PostgreSQL uses
    /// `clock_timestamp()` so time spent waiting for a row lock counts against
    /// leases; SQLite uses its subsecond unix epoch clock.
    fn push_command_ledger_now(builder: &mut QueryBuilder<Self>);

    /// Push the same database clock as epoch seconds for decoding into
    /// [`SystemTime`]. This is distinct from [`Self::push_command_ledger_now`]
    /// because PostgreSQL write/comparison expressions require `timestamptz`
    /// while this framework's portable timestamp decoder consumes `f64`.
    fn push_command_ledger_now_epoch(builder: &mut QueryBuilder<Self>);

    /// Push database-now plus a caller-validated positive duration.
    fn push_command_ledger_deadline(builder: &mut QueryBuilder<Self>, duration: Duration);

    /// Push a JSON value bind, adding the backend's native JSON cast if needed.
    fn push_command_ledger_json(builder: &mut QueryBuilder<Self>, json: &str);

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
    read_model_change_tx: tokio::sync::broadcast::Sender<crate::ReadModelChange>,
    /// When false, skips Postgres `pg_notify` (local broadcast still fires).
    /// Opt-out via [`SqlxRepository::without_read_model_change_notify`]. Writers
    /// that opt out silently break cross-process GraphQL subscriptions.
    notify_enabled: bool,
    projection_change_retention: ProjectionChangeRetention,
    causal_storage_identity: CausalStorageIdentity,
}

impl<DB: sqlx::Database> Clone for SqlxRepository<DB> {
    fn clone(&self) -> Self {
        Self {
            pool: self.pool.clone(),
            read_model_schemas: Arc::clone(&self.read_model_schemas),
            read_model_change_tx: self.read_model_change_tx.clone(),
            notify_enabled: self.notify_enabled,
            projection_change_retention: self.projection_change_retention,
            causal_storage_identity: self.causal_storage_identity,
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
        let (read_model_change_tx, _) = tokio::sync::broadcast::channel(256);
        Self {
            pool,
            read_model_schemas: Arc::new(RwLock::new(TableSchemaRegistry::new())),
            read_model_change_tx,
            notify_enabled: true,
            projection_change_retention: ProjectionChangeRetention::default(),
            causal_storage_identity: CausalStorageIdentity::new(),
        }
    }

    #[cfg_attr(not(feature = "graphql"), allow(dead_code))]
    pub(crate) fn causal_storage_identity(&self) -> CausalStorageIdentity {
        self.causal_storage_identity
    }

    /// Subscribe to read-model table changes (fires after successful write-plan commits).
    ///
    /// Lagging receivers observe [`tokio::sync::broadcast::error::RecvError::Lagged`]
    /// and should treat that as all-dirty for subscription invalidation.
    pub fn read_model_changes(&self) -> tokio::sync::broadcast::Receiver<crate::ReadModelChange> {
        self.read_model_change_tx.subscribe()
    }

    /// Disable Postgres `pg_notify` emission on read-model commits (local broadcast
    /// remains active). Default is ON.
    ///
    /// **Failure mode:** writer processes that opt out silently break
    /// cross-process GraphQL subscriptions that rely on LISTEN/NOTIFY.
    pub fn without_read_model_change_notify(mut self) -> Self {
        self.notify_enabled = false;
        self
    }

    /// Configure the maximum newest projection changes retained per partition.
    ///
    /// Lengthening this value never restores a prefix already represented by
    /// the durable compacted-through watermark.
    pub fn with_projection_change_retention(
        mut self,
        retention: ProjectionChangeRetention,
    ) -> Self {
        self.projection_change_retention = retention;
        self
    }

    pub fn publish_read_model_change(&self, change: crate::ReadModelChange) {
        if change.is_empty() {
            return;
        }
        // Zero receivers is a no-op (broadcast::send returns Err).
        let _ = self.read_model_change_tx.send(change);
    }

    pub(super) fn projection_notify_enabled(&self) -> bool {
        self.notify_enabled
    }

    pub(super) fn projection_change_retention(&self) -> ProjectionChangeRetention {
        self.projection_change_retention
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

impl<DB> CausalGetStream for SqlxRepository<DB>
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
    fn get_causal_stream<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<Option<Entity>, RepositoryError>> + Send + 'a {
        GetStream::get_stream(self, identity)
    }
}

impl<DB> CausalRepositoryIdentity for SqlxRepository<DB>
where
    DB: SqlxRepoBackend,
{
    fn causal_storage_identity(&self) -> CausalStorageIdentity {
        self.causal_storage_identity
    }
}

async fn preflight_command_completion_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    completion: &CommandCompletion,
) -> Result<(), CommandLedgerError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let fence = completion.attempt_fence();

    // SQLite needs a write statement to reserve the database writer before
    // the read; PostgreSQL's subsequent SELECT also carries FOR UPDATE. This
    // establishes one portable lock order before any domain participant is
    // mutated.
    let mut lock = QueryBuilder::<DB>::new(
        "UPDATE command_ledger SET updated_at = updated_at WHERE service_id = ",
    );
    lock.push_bind(fence.key().service_id());
    lock.push(" AND principal_partition = ");
    lock.push_bind(fence.key().principal_partition());
    lock.push(" AND command_id = ");
    lock.push_bind(fence.key().command_id());
    let result =
        lock.build().execute(&mut **tx).await.map_err(|error| {
            repository_storage_error::<DB>("lock command attempt preflight", error)
        })?;
    if DB::rows_affected(&result) != 1 {
        return Err(CommandLedgerError::AttemptFenced {
            command_id: fence.key().command_id().to_string(),
        });
    }

    let record = select_command_ledger_record_in_tx(tx, fence.key(), None)
        .await?
        .ok_or_else(|| CommandLedgerError::AttemptFenced {
            command_id: fence.key().command_id().to_string(),
        })?;
    let now = command_ledger_now_in_tx(tx).await?;
    record.validate_live_attempt(&fence, now)
}

async fn commit_sqlx_batch<'a, DB>(
    repository: &'a SqlxRepository<DB>,
    batch: CommitBatch<'a>,
    mut completion: Option<CommandCompletion>,
    direct_projection: Option<SameTransactionProjectionBatch>,
) -> Result<(), CommandLedgerError>
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
    let prepared = validate_commit_batch(&batch)?;
    for plan in &batch.read_model_plans {
        validate_sql_write_plan(plan).map_err(RepositoryError::from)?;
    }
    if let Some(direct_projection) = &direct_projection {
        direct_projection.validate().map_err(|error| {
            CommandLedgerError::Storage(RepositoryError::Model(error.to_string()))
        })?;
        let completion = completion.as_ref().ok_or_else(|| {
            CommandLedgerError::Invalid(
                "same-transaction direct projection requires a command completion".into(),
            )
        })?;
        if direct_projection.causation_id != completion.attempt().causation_id().as_str() {
            return Err(CommandLedgerError::Invalid(
                "direct projection causation differs from its command attempt".into(),
            ));
        }
    }

    let mut tx = repository
        .pool
        .begin()
        .await
        .map_err(|err| repository_storage_error::<DB>("begin commit transaction", err))?;
    if let Some(completion) = completion.as_ref() {
        preflight_command_completion_in_tx(&mut tx, completion).await?;
    }

    let requested_tables = batch
        .read_model_plans
        .iter()
        .flat_map(|plan| plan.mutations.iter())
        .map(|mutation| mutation.table_name().to_string())
        .collect::<std::collections::BTreeSet<_>>();
    reject_causal_table_writes_in_tx(&mut tx, &requested_tables)
        .await
        .map_err(RepositoryError::from)?;

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
            }
            .into());
        }
    }

    insert_events_in_tx(&repository.pool, &mut tx, &prepared).await?;
    insert_outbox_messages_in_tx(&mut tx, &batch.outbox_messages).await?;

    let mut changed_tables = std::collections::BTreeSet::new();
    for plan in batch.read_model_plans {
        for mutation in &plan.mutations {
            changed_tables.insert(mutation.table_name().to_string());
        }
        apply_read_model_write_plan_in_tx(&mut tx, plan)
            .await
            .map_err(RepositoryError::from)?;
    }

    if let Some(direct_projection) = &direct_projection {
        let evidence = apply_same_transaction_projection_in_tx(
            &mut tx,
            direct_projection,
            repository.projection_change_retention,
        )
        .await
        .map_err(|error| CommandLedgerError::Storage(RepositoryError::Model(error.to_string())))?;
        let completion = completion
            .as_mut()
            .expect("direct projection completion was validated before opening its transaction");
        completion.attach_direct_projection(&evidence)?;
        for mutation in &direct_projection.mutations {
            changed_tables.insert(mutation.mutation.table_name().to_string());
        }
        changed_tables.insert(PROJECTION_CHANGE_NOTIFY_TABLE.to_string());
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

    if repository.notify_enabled && !changed_tables.is_empty() {
        DB::push_change_notify(&mut *tx, &changed_tables)
            .await
            .map_err(RepositoryError::from)?;
    }

    // This fenced terminal update is intentionally the final SQL statement
    // before COMMIT. A stale/expired generation affects zero rows and rolls
    // back every domain write above with the surrounding transaction.
    if let Some(completion) = completion.as_ref() {
        complete_command_in_tx(&mut tx, completion).await?;
    }

    tx.commit()
        .await
        .map_err(|err| repository_storage_error::<DB>("commit transaction", err))?;

    if !changed_tables.is_empty() {
        repository.publish_read_model_change(crate::ReadModelChange {
            tables: changed_tables,
        });
    }
    for stream in batch.streams {
        stream.entity.mark_committed();
    }
    Ok(())
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
            match commit_sqlx_batch(self, batch, None, None).await {
                Ok(()) => Ok(()),
                Err(CommandLedgerError::Storage(error)) => Err(error),
                Err(error) => Err(RepositoryError::Model(format!(
                    "unexpected command ledger error in ordinary commit: {error}"
                ))),
            }
        }
    }
}

impl<DB> CausalTransactionalCommit for SqlxRepository<DB>
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
    fn commit_causal_batch<'a>(
        &'a self,
        batch: CausalCommitBatch<'a>,
    ) -> impl Future<Output = Result<(), CommandLedgerError>> + Send + 'a {
        commit_sqlx_batch(
            self,
            batch.domain,
            Some(batch.completion),
            batch.direct_projection,
        )
    }
}

fn corrupt_ledger_value(error: CommandLedgerError) -> CommandLedgerError {
    CommandLedgerError::Corrupt(error.to_string())
}

#[allow(dead_code)]
fn command_ledger_key_from_row<DB>(row: &DB::Row) -> Result<CommandLedgerKey, CommandLedgerError>
where
    DB: SqlxRepoBackend,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let service_id: String = row.try_get("service_id").map_err(|error| {
        repository_storage_error::<DB>("decode command ledger service ID", error)
    })?;
    let principal: String = row.try_get("principal_partition").map_err(|error| {
        repository_storage_error::<DB>("decode command ledger principal partition", error)
    })?;
    let command_id: String = row.try_get("command_id").map_err(|error| {
        repository_storage_error::<DB>("decode command ledger command ID", error)
    })?;
    CommandLedgerKey::new(
        service_id,
        PrincipalPartitionId::new(principal).map_err(corrupt_ledger_value)?,
        CommandId::parse(command_id).map_err(corrupt_ledger_value)?,
    )
    .map_err(corrupt_ledger_value)
}

fn command_ledger_record_from_row<DB>(
    row: &DB::Row,
    key: CommandLedgerKey,
) -> Result<CommandLedgerRecord, CommandLedgerError>
where
    DB: SqlxRepoBackend,
    for<'q> i64: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let decode = |operation: &'static str, error| repository_storage_error::<DB>(operation, error);
    let command_name: String = row
        .try_get("command_name")
        .map_err(|error| decode("decode command ledger name", error))?;
    let contract: Vec<u8> = row
        .try_get("command_contract_hash")
        .map_err(|error| decode("decode command contract hash", error))?;
    let input: Vec<u8> = row
        .try_get("input_hash")
        .map_err(|error| decode("decode canonical command input hash", error))?;
    let state: String = row
        .try_get("state")
        .map_err(|error| decode("decode command ledger state", error))?;
    let causation_id: String = row
        .try_get("causation_id")
        .map_err(|error| decode("decode command ledger causation ID", error))?;
    let attempt_token: Option<String> = row
        .try_get("attempt_token")
        .map_err(|error| decode("decode command ledger attempt token", error))?;
    let attempt_number: i64 = row
        .try_get("attempt_number")
        .map_err(|error| decode("decode command ledger attempt number", error))?;
    let outcome_json: Option<String> = row
        .try_get("outcome")
        .map_err(|error| decode("decode command ledger outcome", error))?;

    let record = CommandLedgerRecord {
        key,
        command_name,
        contract_fingerprint: CommandContractFingerprint::try_from_slice(&contract)
            .map_err(corrupt_ledger_value)?,
        input_hash: CanonicalInputHash::try_from_slice(&input).map_err(corrupt_ledger_value)?,
        state: CommandLedgerState::parse(&state)?,
        causation_id: CausationId::parse_stored(causation_id)?,
        attempt_token: attempt_token.map(AttemptToken::parse_stored).transpose()?,
        attempt_number: repository_u64_from_i64(
            DB::BACKEND,
            attempt_number,
            "command ledger attempt number",
        )?,
        lease_expires_at: DB::decode_optional_timestamp(row, "lease_expires_at")?,
        outcome_json,
        created_at: DB::decode_timestamp(row, "created_at")?,
        updated_at: DB::decode_timestamp(row, "updated_at")?,
        completed_at: DB::decode_optional_timestamp(row, "completed_at")?,
        retention_expires_at: DB::decode_timestamp(row, "retention_expires_at")?,
        compacted_at: DB::decode_optional_timestamp(row, "compacted_at")?,
    };
    record.validate_stored_shape()?;
    Ok(record)
}

async fn command_ledger_now_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
) -> Result<SystemTime, CommandLedgerError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let mut builder = QueryBuilder::<DB>::new("SELECT ");
    DB::push_command_ledger_now_epoch(&mut builder);
    builder.push(" AS ledger_now");
    let row = builder
        .build()
        .fetch_one(&mut **tx)
        .await
        .map_err(|error| repository_storage_error::<DB>("read command ledger clock", error))?;
    Ok(DB::decode_timestamp(&row, "ledger_now")?)
}

async fn select_command_ledger_record_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    key: &CommandLedgerKey,
    expected_command_name: Option<&str>,
) -> Result<Option<CommandLedgerRecord>, CommandLedgerError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let mut builder = QueryBuilder::<DB>::new("SELECT ");
    builder.push(DB::COMMAND_LEDGER_SELECT);
    builder.push(" FROM command_ledger WHERE service_id = ");
    builder.push_bind(key.service_id());
    builder.push(" AND principal_partition = ");
    builder.push_bind(key.principal_partition());
    builder.push(" AND command_id = ");
    builder.push_bind(key.command_id());
    if let Some(expected_command_name) = expected_command_name {
        builder.push(" AND command_name = ");
        builder.push_bind(expected_command_name);
    }
    builder.push(DB::COMMAND_LEDGER_LOCK_SUFFIX);
    let row = builder
        .build()
        .fetch_optional(&mut **tx)
        .await
        .map_err(|error| repository_storage_error::<DB>("select command ledger row", error))?;
    row.map(|row| command_ledger_record_from_row::<DB>(&row, key.clone()))
        .transpose()
}

async fn insert_command_reservation_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    reservation: &CommandReservation,
) -> Result<bool, CommandLedgerError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB>,
    for<'q> f64: Encode<'q, DB> + Type<DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
{
    let mut builder = QueryBuilder::<DB>::new(
        "INSERT INTO command_ledger (service_id, principal_partition, command_id, \
         command_name, command_contract_hash, input_hash, state, causation_id, attempt_token, \
         attempt_number, lease_expires_at, outcome, created_at, updated_at, completed_at, \
         retention_expires_at, compacted_at) VALUES (",
    );
    builder.push_bind(reservation.key().service_id());
    builder.push(", ");
    builder.push_bind(reservation.key().principal_partition());
    builder.push(", ");
    builder.push_bind(reservation.key().command_id());
    builder.push(", ");
    builder.push_bind(reservation.command_name());
    builder.push(", ");
    builder.push_bind(reservation.contract_fingerprint_bytes().as_slice());
    builder.push(", ");
    builder.push_bind(reservation.input_hash_bytes().as_slice());
    builder.push(", ");
    builder.push_bind(CommandLedgerState::InProgress.as_str());
    builder.push(", ");
    builder.push_bind(reservation.candidate_causation().as_str());
    builder.push(", ");
    builder.push_bind(reservation.candidate_attempt().as_str());
    builder.push(", ");
    builder.push_bind(1_i64);
    builder.push(", ");
    DB::push_command_ledger_deadline(&mut builder, reservation.lease());
    builder.push(", NULL, ");
    DB::push_command_ledger_now(&mut builder);
    builder.push(", ");
    DB::push_command_ledger_now(&mut builder);
    builder.push(", NULL, ");
    DB::push_command_ledger_deadline(&mut builder, reservation.retention());
    builder.push(", NULL");
    builder.push(") ON CONFLICT (service_id, principal_partition, command_id) DO NOTHING");
    let result = builder
        .build()
        .execute(&mut **tx)
        .await
        .map_err(|error| repository_storage_error::<DB>("insert command reservation", error))?;
    Ok(DB::rows_affected(&result) == 1)
}

async fn expire_command_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    key: &CommandLedgerKey,
    require_retention_due: bool,
) -> Result<u64, CommandLedgerError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> String: Encode<'q, DB> + Type<DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
{
    let mut builder = QueryBuilder::<DB>::new(
        "UPDATE command_ledger SET state = 'expired', attempt_token = NULL, \
         lease_expires_at = NULL, outcome = NULL, updated_at = ",
    );
    DB::push_command_ledger_now(&mut builder);
    builder.push(", compacted_at = ");
    DB::push_command_ledger_now(&mut builder);
    builder.push(" WHERE service_id = ");
    builder.push_bind(key.service_id());
    builder.push(" AND principal_partition = ");
    builder.push_bind(key.principal_partition());
    builder.push(" AND command_id = ");
    builder.push_bind(key.command_id());
    builder.push(" AND state <> 'expired'");
    if require_retention_due {
        builder.push(" AND retention_expires_at <= ");
        DB::push_command_ledger_now(&mut builder);
    }
    let result = builder
        .build()
        .execute(&mut **tx)
        .await
        .map_err(|error| repository_storage_error::<DB>("expire command ledger row", error))?;
    Ok(DB::rows_affected(&result))
}

async fn reclaim_command_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    record: &mut CommandLedgerRecord,
    reservation: &CommandReservation,
    now: SystemTime,
) -> Result<(), CommandLedgerError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB>,
    for<'q> f64: Encode<'q, DB> + Type<DB>,
    for<'q> String: Encode<'q, DB> + Type<DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
{
    record.reclaim(reservation, now)?;
    let attempt_number = repository_i64_from_u64(
        DB::BACKEND,
        record.attempt_number,
        "command ledger attempt number",
        DB::INTEGER_STORAGE,
    )?;
    let mut builder = QueryBuilder::<DB>::new(
        "UPDATE command_ledger SET state = 'in_progress', attempt_token = ",
    );
    builder.push_bind(reservation.candidate_attempt().as_str());
    builder.push(", attempt_number = ");
    builder.push_bind(attempt_number);
    builder.push(", lease_expires_at = ");
    DB::push_command_ledger_deadline(&mut builder, reservation.lease());
    builder.push(", outcome = NULL, updated_at = ");
    DB::push_command_ledger_now(&mut builder);
    builder.push(", completed_at = NULL, retention_expires_at = ");
    DB::push_command_ledger_deadline(&mut builder, reservation.retention());
    builder.push(", compacted_at = NULL WHERE service_id = ");
    builder.push_bind(record.key.service_id());
    builder.push(" AND principal_partition = ");
    builder.push_bind(record.key.principal_partition());
    builder.push(" AND command_id = ");
    builder.push_bind(record.key.command_id());
    let result = builder
        .build()
        .execute(&mut **tx)
        .await
        .map_err(|error| repository_storage_error::<DB>("reclaim command attempt", error))?;
    if DB::rows_affected(&result) != 1 {
        return Err(CommandLedgerError::AttemptFenced {
            command_id: record.key.command_id().to_string(),
        });
    }
    Ok(())
}

async fn complete_command_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    completion: &CommandCompletion,
) -> Result<(), CommandLedgerError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB>,
    for<'q> f64: Encode<'q, DB> + Type<DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'q> &'q [u8]: Encode<'q, DB> + Type<DB>,
{
    let fence = completion.attempt_fence();
    let attempt_number = repository_i64_from_u64(
        DB::BACKEND,
        fence.attempt_number(),
        "command ledger attempt number",
        DB::INTEGER_STORAGE,
    )?;
    let terminal_state = CommandLedgerState::from(completion.state()).as_str();
    let mut builder = QueryBuilder::<DB>::new("UPDATE command_ledger SET state = ");
    builder.push_bind(terminal_state);
    builder.push(", attempt_token = NULL, lease_expires_at = NULL, outcome = ");
    DB::push_command_ledger_json(&mut builder, completion.replay_json());
    builder.push(", updated_at = ");
    DB::push_command_ledger_now(&mut builder);
    builder.push(", completed_at = ");
    DB::push_command_ledger_now(&mut builder);
    builder.push(", retention_expires_at = ");
    DB::push_command_ledger_deadline(&mut builder, completion.retention());
    builder.push(", compacted_at = NULL WHERE service_id = ");
    builder.push_bind(fence.key().service_id());
    builder.push(" AND principal_partition = ");
    builder.push_bind(fence.key().principal_partition());
    builder.push(" AND command_id = ");
    builder.push_bind(fence.key().command_id());
    builder.push(" AND command_contract_hash = ");
    builder.push_bind(fence.contract_fingerprint_bytes().as_slice());
    builder.push(" AND input_hash = ");
    builder.push_bind(fence.input_hash_bytes().as_slice());
    builder.push(" AND state = 'in_progress' AND causation_id = ");
    builder.push_bind(fence.causation_id().as_str());
    builder.push(" AND attempt_token = ");
    builder.push_bind(fence.attempt_token().as_str());
    builder.push(" AND attempt_number = ");
    builder.push_bind(attempt_number);
    builder.push(" AND lease_expires_at > ");
    DB::push_command_ledger_now(&mut builder);
    let result =
        builder.build().execute(&mut **tx).await.map_err(|error| {
            repository_storage_error::<DB>("complete command ledger row", error)
        })?;
    if DB::rows_affected(&result) != 1 {
        return Err(CommandLedgerError::AttemptFenced {
            command_id: fence.key().command_id().to_string(),
        });
    }
    Ok(())
}

impl<DB> CommandLedgerStore for SqlxRepository<DB>
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
    fn reserve_command(
        &self,
        reservation: CommandReservation,
    ) -> impl Future<Output = Result<ReservationOutcome, CommandLedgerError>> + Send + '_ {
        async move {
            let mut tx = self.pool.begin().await.map_err(|error| {
                repository_storage_error::<DB>("begin command reservation", error)
            })?;
            if insert_command_reservation_in_tx(&mut tx, &reservation).await? {
                tx.commit().await.map_err(|error| {
                    repository_storage_error::<DB>("commit command reservation", error)
                })?;
                return Ok(ReservationOutcome::Acquired(
                    reservation.acquired_candidate_attempt(),
                ));
            }

            let mut record = select_command_ledger_record_in_tx(&mut tx, reservation.key(), None)
                .await?
                .ok_or_else(|| {
                    CommandLedgerError::Corrupt(format!(
                        "conflicting command `{}` disappeared during reservation",
                        reservation.key().command_id()
                    ))
                })?;
            let now = command_ledger_now_in_tx(&mut tx).await?;
            let decision = record.classify_reservation(&reservation, now)?;
            let outcome = match decision {
                ReservationDecision::Expire => {
                    expire_command_in_tx(&mut tx, reservation.key(), false).await?;
                    ReservationOutcome::Expired
                }
                ReservationDecision::Reclaim => {
                    reclaim_command_in_tx(&mut tx, &mut record, &reservation, now).await?;
                    ReservationOutcome::Acquired(record.acquired_attempt()?)
                }
                other => record.reservation_outcome(other)?,
            };
            tx.commit().await.map_err(|error| {
                repository_storage_error::<DB>("commit command reservation decision", error)
            })?;
            Ok(outcome)
        }
    }

    fn lookup_command<'a>(
        &'a self,
        key: &'a CommandLedgerKey,
        scope: CommandLookupScope<'a>,
    ) -> impl Future<Output = Result<CommandLookup, CommandLedgerError>> + Send + 'a {
        async move {
            let mut tx = self.pool.begin().await.map_err(|error| {
                repository_storage_error::<DB>("begin command ledger lookup", error)
            })?;

            // Establish SQLite's single-writer reservation before selecting;
            // PostgreSQL additionally takes the row lock through its suffix.
            let mut lock = QueryBuilder::<DB>::new(
                "UPDATE command_ledger SET updated_at = updated_at WHERE service_id = ",
            );
            lock.push_bind(key.service_id());
            lock.push(" AND principal_partition = ");
            lock.push_bind(key.principal_partition());
            lock.push(" AND command_id = ");
            lock.push_bind(key.command_id());
            match scope {
                CommandLookupScope::CommandName(expected_command_name)
                | CommandLookupScope::CommandContract {
                    command_name: expected_command_name,
                    ..
                } => {
                    lock.push(" AND command_name = ");
                    lock.push_bind(expected_command_name);
                }
                CommandLookupScope::Attempt(_) => {}
            }
            lock.build().execute(&mut *tx).await.map_err(|error| {
                repository_storage_error::<DB>("lock command ledger lookup", error)
            })?;

            let expected_command_name = match scope {
                CommandLookupScope::CommandName(expected) => Some(expected),
                CommandLookupScope::CommandContract {
                    command_name: expected,
                    ..
                } => Some(expected),
                CommandLookupScope::Attempt(_) => None,
            };
            let Some(mut record) =
                select_command_ledger_record_in_tx(&mut tx, key, expected_command_name).await?
            else {
                tx.commit().await.map_err(|error| {
                    repository_storage_error::<DB>("commit empty command ledger lookup", error)
                })?;
                return Ok(CommandLookup::Unknown);
            };
            if !record.matches_lookup_scope(scope) {
                tx.commit().await.map_err(|error| {
                    repository_storage_error::<DB>("commit mismatched command ledger lookup", error)
                })?;
                return Ok(CommandLookup::Unknown);
            }
            let now = command_ledger_now_in_tx(&mut tx).await?;
            if record.state != CommandLedgerState::Expired && record.retention_expires_at <= now {
                expire_command_in_tx(&mut tx, key, true).await?;
                record.expire(now);
            }
            let lookup = record.lookup()?;
            tx.commit().await.map_err(|error| {
                repository_storage_error::<DB>("commit command ledger lookup", error)
            })?;
            Ok(lookup)
        }
    }

    fn mark_retryable_unknown(
        &self,
        attempt: AttemptFence,
    ) -> impl Future<Output = Result<(), CommandLedgerError>> + Send + '_ {
        async move {
            let attempt_number = repository_i64_from_u64(
                DB::BACKEND,
                attempt.attempt_number(),
                "command ledger attempt number",
                DB::INTEGER_STORAGE,
            )?;
            let mut builder = QueryBuilder::<DB>::new(
                "UPDATE command_ledger SET state = 'retryable_unknown', attempt_token = NULL, \
                 lease_expires_at = NULL, updated_at = ",
            );
            DB::push_command_ledger_now(&mut builder);
            builder.push(" WHERE service_id = ");
            builder.push_bind(attempt.key().service_id());
            builder.push(" AND principal_partition = ");
            builder.push_bind(attempt.key().principal_partition());
            builder.push(" AND command_id = ");
            builder.push_bind(attempt.key().command_id());
            builder.push(" AND command_contract_hash = ");
            builder.push_bind(attempt.contract_fingerprint_bytes().as_slice());
            builder.push(" AND input_hash = ");
            builder.push_bind(attempt.input_hash_bytes().as_slice());
            builder.push(" AND state = 'in_progress' AND causation_id = ");
            builder.push_bind(attempt.causation_id().as_str());
            builder.push(" AND attempt_token = ");
            builder.push_bind(attempt.attempt_token().as_str());
            builder.push(" AND attempt_number = ");
            builder.push_bind(attempt_number);
            let result = builder.build().execute(&self.pool).await.map_err(|error| {
                repository_storage_error::<DB>("mark command retryable unknown", error)
            })?;
            if DB::rows_affected(&result) != 1 {
                return Err(CommandLedgerError::AttemptFenced {
                    command_id: attempt.key().command_id().to_string(),
                });
            }
            Ok(())
        }
    }

    fn compact_expired_commands(
        &self,
        limit: usize,
    ) -> impl Future<Output = Result<u64, CommandLedgerError>> + Send + '_ {
        async move {
            if limit == 0 {
                return Ok(0);
            }
            let limit = i64::try_from(limit).map_err(|_| {
                CommandLedgerError::Invalid("command compaction limit exceeds i64".into())
            })?;
            let mut tx = self.pool.begin().await.map_err(|error| {
                repository_storage_error::<DB>("begin command ledger compaction", error)
            })?;

            // A no-op write obtains SQLite's transaction-wide writer lock.
            // PostgreSQL relies on the per-row SKIP LOCKED suffix below.
            QueryBuilder::<DB>::new(
                "UPDATE command_ledger SET updated_at = updated_at WHERE 1 = 0",
            )
            .build()
            .execute(&mut *tx)
            .await
            .map_err(|error| {
                repository_storage_error::<DB>("lock command ledger compaction", error)
            })?;

            let mut select = QueryBuilder::<DB>::new(
                "SELECT service_id, principal_partition, command_id FROM command_ledger \
                 WHERE state <> 'expired' AND retention_expires_at <= ",
            );
            DB::push_command_ledger_now(&mut select);
            select.push(" ORDER BY retention_expires_at, service_id, principal_partition, command_id LIMIT ");
            select.push_bind(limit);
            select.push(DB::COMMAND_LEDGER_COMPACTION_LOCK_SUFFIX);
            let rows = select.build().fetch_all(&mut *tx).await.map_err(|error| {
                repository_storage_error::<DB>("select command ledger compaction rows", error)
            })?;
            let mut compacted = 0;
            for row in rows {
                let key = command_ledger_key_from_row::<DB>(&row)?;
                compacted += expire_command_in_tx(&mut tx, &key, true).await?;
            }
            tx.commit().await.map_err(|error| {
                repository_storage_error::<DB>("commit command ledger compaction", error)
            })?;
            Ok(compacted)
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
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    fn read_model_capabilities(&self) -> ReadModelAdapterCapabilities {
        sql_read_model_capabilities()
    }

    fn commit_write_plan(
        &self,
        plan: ReadModelWritePlan,
    ) -> impl Future<Output = Result<ReadModelCommitOutcome, ReadModelError>> + Send + '_ {
        async move {
            let tables: std::collections::BTreeSet<String> = plan
                .mutations
                .iter()
                .map(|m| m.table_name().to_string())
                .collect();
            validate_sql_write_plan(&plan)?;
            let mut tx = begin_read_model_tx(&self.pool).await?;
            reject_causal_table_writes_in_tx(&mut tx, &tables).await?;
            let outcome = apply_read_model_write_plan_in_tx(&mut tx, plan).await?;
            if self.notify_enabled && !tables.is_empty() {
                DB::push_change_notify(&mut *tx, &tables).await?;
            }
            commit_read_model_tx(tx).await?;
            if outcome.was_applied() && !tables.is_empty() {
                self.publish_read_model_change(crate::ReadModelChange { tables });
            }
            Ok(outcome)
        }
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

    fn backlog_stats(
        &self,
    ) -> impl Future<Output = Result<OutboxBacklogStats, RepositoryError>> + Send + '_ {
        async move {
            let mut builder = QueryBuilder::<DB>::new("SELECT COUNT(*) AS pending_count, ");
            builder.push(DB::OUTBOX_OLDEST_CREATED_AT_SELECT);
            builder.push(" FROM outbox_messages WHERE status = ");
            builder.push_bind(OutboxMessageStatus::Pending.as_str());
            let row =
                builder.build().fetch_one(&self.pool).await.map_err(|err| {
                    repository_storage_error::<DB>("load outbox backlog stats", err)
                })?;

            let pending_count: i64 = row.try_get("pending_count").map_err(|err| {
                repository_storage_error::<DB>("decode outbox backlog count row", err)
            })?;
            let pending =
                repository_u64_from_i64(DB::BACKEND, pending_count, "outbox backlog count")
                    .and_then(|value| {
                        usize::try_from(value).map_err(|_| {
                            RepositoryError::Model(format!(
                                "{} outbox backlog count value {value} is invalid",
                                DB::BACKEND
                            ))
                        })
                    })?;
            let oldest_created_at = DB::decode_optional_timestamp(&row, "oldest_created_at")?;

            Ok(OutboxBacklogStats {
                pending,
                oldest_created_at,
            })
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
