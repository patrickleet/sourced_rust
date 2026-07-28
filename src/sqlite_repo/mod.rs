//! SQLite backend for the shared SQLx repository.
//!
//! The event-store/snapshot/outbox/inbox logic lives once in
//! [`crate::sqlx_repo::repo`]; this module carries only what is genuinely
//! SQLite-specific: the schema SQL, the `"secs.nanos"` text timestamp codec,
//! bind-parameter chunking, the unique-constraint predicate, and the
//! candidate-scan outbox claim (SQLite has no row locks). It is feature-gated
//! behind `sqlite` and async-only.

use std::sync::LazyLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sqlx::migrate::Migrator;
use sqlx::query_builder::Separated;
use sqlx::sqlite::SqliteRow;
use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool};

use crate::outbox::{OutboxMessage, OutboxMessageStatus};
use crate::outbox_worker::ClaimOutboxMessages;
use crate::repository::RepositoryError;
use crate::sqlx_repo::read_model::quote_identifier;
use crate::sqlx_repo::repo::{
    SqlxOutboxStore, SqlxRepository, embedded_migrator, outbox_message_by_id,
    system_time_epoch_secs,
};
use crate::sqlx_repo::{
    self, is_sqlite_unique_constraint, read_model_i64_from_u64 as sqlx_read_model_i64_from_u64,
    read_model_u64_from_i64 as sqlx_read_model_u64_from_i64,
    repository_i64_from_u64 as sqlx_repository_i64_from_u64,
};
use crate::table::TableSqlDialect;
use crate::table::{
    ColumnType, RowValue, TableColumn as ColumnDef, TableStoreError as ReadModelError,
};

static SQLITE_MIGRATOR: LazyLock<Migrator> = LazyLock::new(|| {
    embedded_migrator(&[
        (
            1,
            "initial",
            include_str!("../../migrations/sqlite/0001_initial.sql"),
        ),
        (
            2,
            "command ledger",
            include_str!("../../migrations/sqlite/0002_command_ledger.sql"),
        ),
        (
            3,
            "projection protocol",
            include_str!("../../migrations/sqlite/0003_projection_protocol.sql"),
        ),
    ])
});
const SQLITE_BACKEND: &str = "sqlite";
const SIGNED_INTEGER_STORAGE: &str = "signed integer storage";

/// SQLite-backed async repository.
pub type SqliteRepository = SqlxRepository<Sqlite>;

/// SQLite-backed outbox table store.
pub type SqliteOutboxStore = SqlxOutboxStore<Sqlite>;

impl crate::sqlx_repo::repo::SqlxRepoBackend for Sqlite {
    fn migrator() -> &'static Migrator {
        &SQLITE_MIGRATOR
    }
    // SQLite's historical bound-parameter limit is 999; staying under it keeps
    // the batched inserts portable across SQLite builds.
    const MAX_BIND_PARAMS: usize = 900;
    // SQLite does not abort the transaction on a constraint error, so conflict
    // recovery re-reads stream versions in the same transaction.
    const CONFLICT_REREAD_IN_TX: bool = true;
    const NOW: &'static str = "CURRENT_TIMESTAMP";
    const COMMAND_LEDGER_SELECT: &'static str = "command_name, command_contract_hash, \
         input_hash, state, causation_id, attempt_token, attempt_number, lease_expires_at, \
         outcome, created_at, updated_at, completed_at, retention_expires_at, compacted_at";
    const COMMAND_LEDGER_LOCK_SUFFIX: &'static str = "";
    const COMMAND_LEDGER_COMPACTION_LOCK_SUFFIX: &'static str = "";
    const EVENT_SELECT: &'static str = "event_name, event_version, payload, payload_codec, \
         payload_codec_version, metadata, sequence, recorded_at";
    const SNAPSHOT_SELECT: &'static str = "aggregate_type, aggregate_id, version, \
         snapshot_version, payload, payload_codec, payload_codec_version, metadata, recorded_at";
    const OUTBOX_SELECT: &'static str = "message_id, event_type, payload, payload_codec, \
         payload_codec_version, metadata, status, created_at, claimed_by, claimed_until, \
         attempts, last_error, destination, source_aggregate_type, source_aggregate_id, \
         source_sequence, correlation_id, causation_id";
    const ORDER_BY_CREATED_AT: &'static str = "CAST(created_at AS REAL)";
    const OUTBOX_OLDEST_CREATED_AT_SELECT: &'static str =
        "MIN(CAST(created_at AS REAL)) AS oldest_created_at";
    const TABLE_DIALECT: TableSqlDialect = TableSqlDialect::Sqlite;

    /// `"secs.nanos"` text, sortable/comparable via `CAST(... AS REAL)`.
    type TimestampValue = String;

    fn default_pool_size(database_url: &str) -> u32 {
        if database_url.contains(":memory:") {
            1
        } else {
            5
        }
    }

    fn is_unique_violation(err: &sqlx::Error) -> bool {
        is_sqlite_unique_constraint(err)
    }

    fn timestamp_value(timestamp: SystemTime) -> Result<String, RepositoryError> {
        system_time_to_storage(timestamp)
    }

    fn push_timestamp(sep: &mut Separated<'_, Sqlite, &'static str>, value: &String) {
        sep.push_bind(value.as_str());
    }

    fn push_optional_timestamp(
        sep: &mut Separated<'_, Sqlite, &'static str>,
        value: Option<&String>,
    ) {
        sep.push_bind(value.map(String::as_str));
    }

    fn push_timestamp_assign(builder: &mut QueryBuilder<Sqlite>, value: &String) {
        builder.push_bind(value.as_str());
    }

    fn push_timestamp_cmp(
        builder: &mut QueryBuilder<Sqlite>,
        column: &'static str,
        op: &'static str,
        epoch_secs: f64,
    ) {
        builder.push("CAST(");
        builder.push(column);
        builder.push(" AS REAL) ");
        builder.push(op);
        builder.push(" ");
        builder.push_bind(epoch_secs);
    }

    fn push_command_ledger_now(builder: &mut QueryBuilder<Sqlite>) {
        builder.push("unixepoch('now','subsec')");
    }

    fn push_command_ledger_now_epoch(builder: &mut QueryBuilder<Sqlite>) {
        builder.push("unixepoch('now','subsec')");
    }

    fn push_command_ledger_deadline(builder: &mut QueryBuilder<Sqlite>, duration: Duration) {
        builder.push("(unixepoch('now','subsec') + ");
        builder.push_bind(duration.as_secs_f64());
        builder.push(")");
    }

    fn push_command_ledger_deadline_is_live(builder: &mut QueryBuilder<Sqlite>, deadline: &String) {
        builder.push("CAST(");
        builder.push_bind(deadline.as_str());
        builder.push(" AS REAL) > unixepoch('now','subsec')");
    }

    fn push_command_ledger_json(builder: &mut QueryBuilder<Sqlite>, json: &str) {
        builder.push_bind(json);
    }

    fn decode_timestamp(
        row: &SqliteRow,
        column: &'static str,
    ) -> Result<SystemTime, RepositoryError> {
        match row.try_get::<String, _>(column) {
            Ok(value) => system_time_from_storage(&value),
            Err(string_err) => {
                let value = row.try_get::<f64, _>(column).map_err(|float_err| {
                    RepositoryError::Model(format!(
                        "decode {column} timestamp row failed as sqlite text ({string_err}) and real ({float_err})"
                    ))
                })?;
                system_time_from_epoch_secs(value)
            }
        }
    }

    fn decode_optional_timestamp(
        row: &SqliteRow,
        column: &'static str,
    ) -> Result<Option<SystemTime>, RepositoryError> {
        match row.try_get::<Option<String>, _>(column) {
            Ok(value) => value.as_deref().map(system_time_from_storage).transpose(),
            Err(string_err) => row
                .try_get::<Option<f64>, _>(column)
                .map_err(|float_err| {
                    RepositoryError::Model(format!(
                        "decode {column} timestamp row failed as sqlite text ({string_err}) and real ({float_err})"
                    ))
                })?
                .map(system_time_from_epoch_secs)
                .transpose(),
        }
    }

    fn push_metadata(sep: &mut Separated<'_, Sqlite, &'static str>, json: &str) {
        sep.push_bind(json);
    }

    fn push_id_filter(builder: &mut QueryBuilder<Sqlite>, ids: &[&str]) {
        // SQLite has no array type, so the id list is built as bound
        // placeholders.
        builder.push("aggregate_id IN (");
        {
            let mut separated = builder.separated(", ");
            for id in ids {
                separated.push_bind(*id);
            }
        }
        builder.push(")");
    }

    fn inbox_purge_query(age: Duration) -> QueryBuilder<Sqlite> {
        // `processed_at` defaults to CURRENT_TIMESTAMP (UTC `YYYY-MM-DD
        // HH:MM:SS`), so compare against the database clock via
        // `datetime('now', '-N seconds')`.
        let mut builder =
            QueryBuilder::new("DELETE FROM consumer_inbox WHERE processed_at < datetime('now', ");
        builder.push_bind(format!("-{} seconds", age.as_secs()));
        builder.push(")");
        builder
    }

    async fn claim_outbox(
        pool: &SqlitePool,
        request: ClaimOutboxMessages,
    ) -> Result<Vec<OutboxMessage>, RepositoryError> {
        {
            if request.batch_size == 0 {
                return Ok(Vec::new());
            }

            let now = SystemTime::now();
            let now_epoch = system_time_epoch_secs::<Sqlite>(now)?;
            let claimed_until = now.checked_add(request.lease).ok_or_else(|| {
                RepositoryError::Model("failed to compute outbox lease deadline".into())
            })?;
            let claimed_until_storage = system_time_to_storage(claimed_until)?;

            let mut tx = pool
                .begin()
                .await
                .map_err(|err| repository_storage_error("begin outbox claim transaction", err))?;

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

                if let Some(message) = outbox_message_by_id(&mut *tx, &message_id).await? {
                    claimed.push(message);
                }
            }

            tx.commit()
                .await
                .map_err(|err| repository_storage_error("commit outbox claim transaction", err))?;
            Ok(claimed)
        }
    }
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
                    "read model `{}` column `{}` has unsupported type `{}`",
                    column.field_name, column.column_name, type_name
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

fn system_time_from_storage(value: &str) -> Result<SystemTime, RepositoryError> {
    let invalid =
        || RepositoryError::Model(format!("sqlite stored timestamp `{value}` is invalid"));
    let (secs, nanos) = value.split_once('.').ok_or_else(invalid)?;
    let secs = secs.parse::<u64>().map_err(|_| invalid())?;
    let nanos = nanos.parse::<u32>().map_err(|_| invalid())?;
    if nanos >= 1_000_000_000 {
        return Err(invalid());
    }
    Ok(UNIX_EPOCH + Duration::new(secs, nanos))
}

fn system_time_from_epoch_secs(value: f64) -> Result<SystemTime, RepositoryError> {
    if !value.is_finite() || value < 0.0 {
        return Err(RepositoryError::Model(format!(
            "sqlite timestamp epoch value {value} is invalid"
        )));
    }
    Ok(UNIX_EPOCH + Duration::from_secs_f64(value))
}

fn repository_storage_error(operation: &str, err: sqlx::Error) -> RepositoryError {
    sqlx_repo::repository_storage_error(SQLITE_BACKEND, operation, err)
}

fn read_model_storage_error(operation: &str, err: sqlx::Error) -> ReadModelError {
    sqlx_repo::read_model_storage_error(SQLITE_BACKEND, operation, err)
}
