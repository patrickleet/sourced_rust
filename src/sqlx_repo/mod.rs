use std::collections::HashMap;

#[cfg(any(feature = "postgres", feature = "sqlite"))]
use crate::read_model::ReadModelError;
use crate::repository::RepositoryError;

pub(crate) fn serialize_event_metadata(
    metadata: &HashMap<String, String>,
) -> Result<String, RepositoryError> {
    serde_json::to_string(metadata)
        .map_err(|err| RepositoryError::Model(format!("serialize event metadata: {err}")))
}

pub(crate) fn deserialize_event_metadata(
    metadata_json: &str,
) -> Result<HashMap<String, String>, RepositoryError> {
    serde_json::from_str(metadata_json)
        .map_err(|err| RepositoryError::Model(format!("deserialize event metadata: {err}")))
}

pub(crate) fn repository_i64_from_u64(
    backend: &str,
    value: u64,
    field: &str,
    storage: &str,
) -> Result<i64, RepositoryError> {
    i64::try_from(value).map_err(|_| {
        RepositoryError::Model(format!("{backend} {field} value {value} exceeds {storage}"))
    })
}

#[cfg(feature = "postgres")]
pub(crate) fn repository_i32_from_u64(
    backend: &str,
    value: u64,
    field: &str,
    storage: &str,
) -> Result<i32, RepositoryError> {
    i32::try_from(value).map_err(|_| {
        RepositoryError::Model(format!("{backend} {field} value {value} exceeds {storage}"))
    })
}

pub(crate) fn repository_u64_from_i64(
    backend: &str,
    value: i64,
    field: &str,
) -> Result<u64, RepositoryError> {
    u64::try_from(value)
        .map_err(|_| RepositoryError::Model(format!("{backend} {field} value {value} is negative")))
}

#[cfg(feature = "postgres")]
pub(crate) fn repository_u64_from_i32(
    backend: &str,
    value: i32,
    field: &str,
) -> Result<u64, RepositoryError> {
    u64::try_from(value)
        .map_err(|_| RepositoryError::Model(format!("{backend} {field} value {value} is negative")))
}

#[cfg(feature = "sqlite")]
pub(crate) fn repository_u16_from_i64(
    backend: &str,
    value: i64,
    field: &str,
) -> Result<u16, RepositoryError> {
    u16::try_from(value)
        .map_err(|_| RepositoryError::Model(format!("{backend} {field} value {value} is invalid")))
}

#[cfg(feature = "postgres")]
pub(crate) fn repository_u16_from_i32(
    backend: &str,
    value: i32,
    field: &str,
) -> Result<u16, RepositoryError> {
    u16::try_from(value)
        .map_err(|_| RepositoryError::Model(format!("{backend} {field} value {value} is invalid")))
}

#[cfg(any(feature = "postgres", feature = "sqlite"))]
pub(crate) fn read_model_i64_from_u64(
    backend: &str,
    value: u64,
    field: &str,
    storage: &str,
) -> Result<i64, ReadModelError> {
    i64::try_from(value).map_err(|_| {
        ReadModelError::Storage(format!("{backend} {field} value {value} exceeds {storage}"))
    })
}

#[cfg(any(feature = "postgres", feature = "sqlite"))]
pub(crate) fn read_model_u64_from_i64(
    backend: &str,
    value: i64,
    field: &str,
) -> Result<u64, ReadModelError> {
    u64::try_from(value).map_err(|_| {
        ReadModelError::Storage(format!("{backend} {field} value {value} is negative"))
    })
}

#[cfg(any(feature = "postgres", feature = "sqlite"))]
pub(crate) fn audited_table_schema_sql(statement: String) -> sqlx::AssertSqlSafe<String> {
    // table_schema_statements validates the registry and quotes identifiers before
    // rendering DDL. Schema-authored SQL defaults are the only raw fragments.
    sqlx::AssertSqlSafe(statement)
}

#[cfg(feature = "sqlite")]
pub(crate) fn is_sqlite_unique_constraint(err: &sqlx::Error) -> bool {
    match err {
        sqlx::Error::Database(db_err) => {
            let message = db_err.message();
            let code = db_err.code().map(|code| code.into_owned());
            message.contains("UNIQUE constraint failed")
                || message.contains("PRIMARY KEY")
                || matches!(code.as_deref(), Some("1555" | "2067"))
        }
        _ => false,
    }
}

/// Whether a SQLite error is a transient "database is locked"/"busy" condition.
///
/// SQLite serializes writers; without a `busy_timeout` a colliding writer gets
/// `SQLITE_BUSY` (5) / `SQLITE_LOCKED` (6) immediately. For the lease lock that
/// is contention, not failure, so the acquire loop should retry rather than
/// surface it as a `LockError`. Also treats a pool-acquire timeout (e.g. the
/// single-connection `:memory:` pool under contention) as retryable.
#[cfg(feature = "sqlite")]
pub(crate) fn is_sqlite_busy(err: &sqlx::Error) -> bool {
    match err {
        sqlx::Error::Database(db_err) => {
            let message = db_err.message().to_ascii_lowercase();
            let code = db_err.code().map(|code| code.into_owned());
            message.contains("database is locked")
                || message.contains("database table is locked")
                || matches!(
                    code.as_deref(),
                    Some("5" | "6" | "261" | "262" | "517" | "518")
                )
        }
        sqlx::Error::PoolTimedOut => true,
        _ => false,
    }
}

#[cfg(feature = "postgres")]
pub(crate) fn is_postgres_unique_violation(err: &sqlx::Error) -> bool {
    match err {
        sqlx::Error::Database(db_err) => db_err.code().as_deref() == Some("23505"),
        _ => false,
    }
}

/// Whether a `sqlx::Error` represents a transient condition worth retrying.
///
/// Connection loss, pool exhaustion, and acquire/I/O timeouts are infrastructure
/// hiccups: the same statement may succeed once the backend recovers. SQLite
/// `SQLITE_BUSY`/`SQLITE_LOCKED` contention is likewise transient. Everything
/// else — most notably a `Database` error such as a constraint violation or a
/// malformed-row decode — is deterministic: re-running the identical statement
/// against the same data cannot change the outcome, so it is classified
/// permanent. Treating an unknown failure as permanent is the safe default: a
/// permanent classification hands the message to the failure policy instead of
/// redelivering it forever.
pub(crate) fn is_sqlx_transient(err: &sqlx::Error) -> bool {
    // Connection / pool / timeout failures are transient regardless of backend.
    if matches!(
        err,
        sqlx::Error::PoolTimedOut | sqlx::Error::PoolClosed | sqlx::Error::Io(_)
    ) {
        return true;
    }
    // SQLite serializes writers; busy/locked contention is retryable, not failure.
    #[cfg(feature = "sqlite")]
    if is_sqlite_busy(err) {
        return true;
    }
    // Postgres serialization_failure (40001) / deadlock_detected (40P01): the
    // transaction lost a write race and should be retried, not handed to the
    // failure policy. SQLite never carries these SQLSTATEs, so no feature gate.
    if let sqlx::Error::Database(db_err) = err {
        if matches!(db_err.code().as_deref(), Some("40001" | "40P01")) {
            return true;
        }
    }
    false
}

pub(crate) fn repository_storage_error(
    backend: &str,
    operation: &str,
    err: sqlx::Error,
) -> RepositoryError {
    let retryable = is_sqlx_transient(&err);
    RepositoryError::Storage {
        operation: format!("{backend} {operation}"),
        retryable,
        source: Some(Box::new(err)),
    }
}

#[cfg(any(feature = "postgres", feature = "sqlite"))]
pub(crate) fn read_model_storage_error(
    backend: &str,
    operation: &str,
    err: sqlx::Error,
) -> ReadModelError {
    ReadModelError::Storage(format!("{backend} {operation} failed: {err}"))
}
