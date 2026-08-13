use sqlx::{Database, Executor, QueryBuilder};

use crate::table::{RowValue, TableColumn, TableStoreError};

/// Dialect surface for the shared relational write path.
///
/// The upsert/patch/delete SQL is identical across Postgres and SQLite because
/// `QueryBuilder<DB>` renders the right placeholder dialect; only two things
/// genuinely differ: the value-binding for `Bool`/typed-`NULL` (Postgres binds
/// native `bool` and needs explicit per-type `NULL` casts for `$N` inference;
/// SQLite stores booleans as `i64` and collapses integer/bool `NULL`s) and the
/// backend/storage labels used in numeric-conversion error messages. Those —
/// and nothing else — live behind this trait, implemented once per backend.
pub trait SqlxReadModelBackend: Database {
    /// Backend name used in numeric-conversion error messages (`"postgres"`/`"sqlite"`).
    const BACKEND: &'static str;
    /// Human-readable storage label for the signed-64-bit version column.
    const INTEGER_STORAGE: &'static str;

    /// Bind one `RowValue` into the builder (dialect-specific encoding), then push
    /// any required type cast (Postgres `::jsonb`/`::timestamptz`; SQLite none).
    fn push_row_value_bind(
        builder: &mut QueryBuilder<Self>,
        value: RowValue,
        column: &TableColumn,
    ) -> Result<(), TableStoreError>;

    /// Bind a typed `NULL` for the column's type (Postgres needs the concrete
    /// `Option::<T>::None` per type so `$N` infers correctly).
    fn push_null_bind(
        builder: &mut QueryBuilder<Self>,
        column: &TableColumn,
    ) -> Result<(), TableStoreError>;

    /// Affected-row count of a write result. `sqlx` exposes `rows_affected` only as
    /// an inherent method on each backend's `QueryResult`, not via a shared trait,
    /// so the one-line accessor is delegated here.
    fn rows_affected(result: &Self::QueryResult) -> u64;

    /// Render one `SELECT`-list column. Postgres casts JSON/Timestamp to `::text`
    /// so they decode as `String`; SQLite stores them as text already, so it just
    /// pushes the quoted column. (Reading is the inverse of `push_row_value_bind`.)
    fn push_select_column(builder: &mut QueryBuilder<Self>, column: &TableColumn);

    /// Decode one fetched column into a `RowValue`. The one genuinely dialect-
    /// specific read: Postgres has a native `BOOLEAN`, SQLite stores booleans as
    /// `INTEGER` and decodes `value != 0`.
    fn row_value(row: &Self::Row, column: &TableColumn) -> Result<RowValue, TableStoreError>;

    /// Emit a dialect-specific change notification inside an open transaction
    /// (Postgres: `pg_notify`; default: no-op). Delivery happens on commit.
    fn push_change_notify<'e, E>(
        _executor: E,
        _tables: &std::collections::BTreeSet<String>,
    ) -> impl std::future::Future<Output = Result<(), TableStoreError>> + Send
    where
        E: Executor<'e, Database = Self> + Send,
    {
        async { Ok(()) }
    }
}
