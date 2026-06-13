//! Backend-agnostic read-model logic shared by the Postgres and SQLite repositories.
//!
//! Two layers live here:
//!
//! 1. **Pure helpers** (no `sqlx` types): write-plan validation, key/patch
//!    reconciliation, row-version arithmetic, relationship-include resolution.
//! 2. **The generic relational write path**: free functions over
//!    `DB: SqlxReadModelBackend` that build and run the upsert/patch/delete SQL via
//!    `QueryBuilder<DB>`. Only the value-binding (`Bool`/typed-`NULL`), the
//!    `rows_affected` accessor, and two label strings differ per dialect; the
//!    [`SqlxReadModelBackend`] trait carries exactly those — one trait, two
//!    one-line-per-method impls — so the SQL-building logic exists once rather than
//!    being mirrored (and risking silent drift) in `postgres_repo`/`sqlite_repo`.

use std::sync::RwLock;

use sqlx::{Database, Encode, Executor, IntoArguments, QueryBuilder, Row, Transaction, Type};

use crate::read_model::{
    key_fingerprint, validate_key, validate_row_values, ColumnDef, DeleteRowMutation,
    ExpectedVersion, PatchMode, PatchRowMutation, ReadModelAdapterCapabilities,
    ReadModelCommitOutcome, ReadModelError, ReadModelLoadRequest, ReadModelMutation,
    ReadModelSchema, ReadModelWritePlan, RelationshipDef, RelationshipKind, RowKey, RowMutation,
    RowValue, RowValues, RowWriteMode,
};
use crate::table::TableSchemaRegistry;

/// A resolved relationship include: the relationship metadata plus the registered
/// schema of the target model, ready for the relational load path to query.
#[derive(Clone)]
pub(crate) struct IncludeSpec {
    pub(crate) name: String,
    pub(crate) relationship: RelationshipDef,
    pub(crate) target_schema: ReadModelSchema,
}

pub(crate) fn remember_read_model_schemas(
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

pub(crate) fn resolve_registered_read_model_schemas(
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

pub(crate) fn sql_read_model_capabilities() -> ReadModelAdapterCapabilities {
    ReadModelAdapterCapabilities {
        relational_rows: true,
        sparse_patches: true,
        deletes: true,
    }
}

pub(crate) fn validate_sql_write_plan(plan: &ReadModelWritePlan) -> Result<(), ReadModelError> {
    plan.validate_for(&sql_read_model_capabilities())
}

pub(crate) fn initial_row_version() -> u64 {
    1
}

pub(crate) fn next_row_version(
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

pub(crate) fn validate_row_expected_version(
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

pub(crate) fn row_concurrency_conflict(
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

pub(crate) fn row_values_from_key_and_patch(
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

pub(crate) fn patch_values_preserving_key<'schema>(
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

pub(crate) fn validate_values_match_key(
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

pub(crate) fn belongs_to_target_column(
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

pub(crate) fn empty_string_as_none(value: &str) -> Option<&str> {
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

pub(crate) fn row_write_values<'schema>(
    schema: &'schema ReadModelSchema,
    values: &RowValues,
) -> Result<Vec<(&'schema ColumnDef, RowValue)>, ReadModelError> {
    values
        .iter()
        .map(|(column_name, value)| Ok((column_by_name(schema, column_name)?, value.clone())))
        .collect()
}

pub(crate) fn column_by_name<'schema>(
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

pub(crate) fn version_column(schema: &ReadModelSchema) -> Result<&str, ReadModelError> {
    schema.version_column.as_deref().ok_or_else(|| {
        ReadModelError::Metadata(format!(
            "read model `{}` requires a version column for SQL write-plan persistence",
            schema.model_name
        ))
    })
}

pub(crate) fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

use super::{read_model_i64_from_u64, read_model_storage_error, read_model_u64_from_i64};

/// Dialect surface for the shared relational write path.
///
/// The upsert/patch/delete SQL is identical across Postgres and SQLite because
/// `QueryBuilder<DB>` renders the right placeholder dialect; only two things
/// genuinely differ: the value-binding for `Bool`/typed-`NULL` (Postgres binds
/// native `bool` and needs explicit per-type `NULL` casts for `$N` inference;
/// SQLite stores booleans as `i64` and collapses integer/bool `NULL`s) and the
/// backend/storage labels used in numeric-conversion error messages. Those —
/// and nothing else — live behind this trait, implemented once per backend.
pub(crate) trait SqlxReadModelBackend: Database {
    /// Backend name used in numeric-conversion error messages (`"postgres"`/`"sqlite"`).
    const BACKEND: &'static str;
    /// Human-readable storage label for the signed-64-bit version column.
    const INTEGER_STORAGE: &'static str;

    /// Bind one `RowValue` into the builder (dialect-specific encoding), then push
    /// any required type cast (Postgres `::jsonb`/`::timestamptz`; SQLite none).
    fn push_row_value_bind(
        builder: &mut QueryBuilder<Self>,
        value: RowValue,
        column: &ColumnDef,
    ) -> Result<(), ReadModelError>;

    /// Bind a typed `NULL` for the column's type (Postgres needs the concrete
    /// `Option::<T>::None` per type so `$N` infers correctly).
    fn push_null_bind(
        builder: &mut QueryBuilder<Self>,
        column: &ColumnDef,
    ) -> Result<(), ReadModelError>;

    /// Affected-row count of a write result. `sqlx` exposes `rows_affected` only as
    /// an inherent method on each backend's `QueryResult`, not via a shared trait,
    /// so the one-line accessor is delegated here.
    fn rows_affected(result: &Self::QueryResult) -> u64;
}

pub(crate) async fn begin_read_model_tx<DB: SqlxReadModelBackend>(
    pool: &sqlx::Pool<DB>,
) -> Result<Transaction<'_, DB>, ReadModelError> {
    pool.begin()
        .await
        .map_err(|err| read_model_storage_error(DB::BACKEND, "begin transaction", err))
}

pub(crate) async fn commit_read_model_tx<DB: SqlxReadModelBackend>(
    tx: Transaction<'_, DB>,
) -> Result<(), ReadModelError> {
    tx.commit()
        .await
        .map_err(|err| read_model_storage_error(DB::BACKEND, "commit transaction", err))
}

pub(crate) async fn apply_read_model_write_plan_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    plan: ReadModelWritePlan,
) -> Result<ReadModelCommitOutcome, ReadModelError>
where
    DB: SqlxReadModelBackend,
    for<'c> &'c mut <DB as Database>::Connection: Executor<'c, Database = DB>,
    <DB as Database>::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'r> &'r str: sqlx::ColumnIndex<<DB as Database>::Row>,
{
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

pub(crate) async fn upsert_relational_row_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    mutation: RowMutation,
) -> Result<(), ReadModelError>
where
    DB: SqlxReadModelBackend,
    for<'c> &'c mut <DB as Database>::Connection: Executor<'c, Database = DB>,
    <DB as Database>::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'r> &'r str: sqlx::ColumnIndex<<DB as Database>::Row>,
{
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

pub(crate) async fn patch_relational_row_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    mutation: PatchRowMutation,
) -> Result<(), ReadModelError>
where
    DB: SqlxReadModelBackend,
    for<'c> &'c mut <DB as Database>::Connection: Executor<'c, Database = DB>,
    <DB as Database>::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'r> &'r str: sqlx::ColumnIndex<<DB as Database>::Row>,
{
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

pub(crate) async fn delete_relational_row_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    mutation: DeleteRowMutation,
) -> Result<(), ReadModelError>
where
    DB: SqlxReadModelBackend,
    for<'c> &'c mut <DB as Database>::Connection: Executor<'c, Database = DB>,
    <DB as Database>::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'r> &'r str: sqlx::ColumnIndex<<DB as Database>::Row>,
{
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

pub(crate) async fn row_version_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    schema: &ReadModelSchema,
    key: &RowKey,
) -> Result<Option<u64>, ReadModelError>
where
    DB: SqlxReadModelBackend,
    for<'c> &'c mut <DB as Database>::Connection: Executor<'c, Database = DB>,
    <DB as Database>::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'r> &'r str: sqlx::ColumnIndex<<DB as Database>::Row>,
{
    let version_column = version_column(schema)?;
    let mut builder = QueryBuilder::<DB>::new("SELECT ");
    builder.push(quote_identifier(version_column));
    builder.push(" FROM ");
    builder.push(quote_identifier(&schema.table_name));
    push_key_predicates(&mut builder, schema, key)?;

    let row = builder
        .build()
        .fetch_optional(&mut **tx)
        .await
        .map_err(|err| read_model_storage_error(DB::BACKEND, "load relational row version", err))?;

    row.map(|row| {
        read_model_u64_from_i64(
            DB::BACKEND,
            row.try_get::<i64, _>(version_column).map_err(|err| {
                read_model_storage_error(DB::BACKEND, "decode relational row version", err)
            })?,
            version_column,
        )
    })
    .transpose()
}

pub(crate) async fn insert_relational_row_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    schema: &ReadModelSchema,
    values: &RowValues,
    version: u64,
) -> Result<(), ReadModelError>
where
    DB: SqlxReadModelBackend,
    for<'c> &'c mut <DB as Database>::Connection: Executor<'c, Database = DB>,
    <DB as Database>::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'r> &'r str: sqlx::ColumnIndex<<DB as Database>::Row>,
{
    let version_column = version_column(schema)?;
    let write_values = row_write_values(schema, values)?;
    let has_write_values = !write_values.is_empty();
    let mut builder = QueryBuilder::<DB>::new("INSERT INTO ");
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
        DB::push_row_value_bind(&mut builder, value, column)?;
    }
    if has_write_values {
        builder.push(", ");
    }
    builder.push_bind(read_model_i64_from_u64(
        DB::BACKEND,
        version,
        version_column,
        DB::INTEGER_STORAGE,
    )?);
    builder.push(")");

    builder
        .build()
        .execute(&mut **tx)
        .await
        .map_err(|err| read_model_storage_error(DB::BACKEND, "insert relational row", err))?;

    Ok(())
}

pub(crate) async fn update_relational_row_values_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    schema: &ReadModelSchema,
    key: &RowKey,
    values: &RowValues,
    expected_version: u64,
    version: u64,
) -> Result<u64, ReadModelError>
where
    DB: SqlxReadModelBackend,
    for<'c> &'c mut <DB as Database>::Connection: Executor<'c, Database = DB>,
    <DB as Database>::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'r> &'r str: sqlx::ColumnIndex<<DB as Database>::Row>,
{
    let mut write_values = row_write_values(schema, values)?;
    write_values.retain(|(column, _)| !column.primary_key);
    update_relational_columns_in_tx(tx, schema, key, write_values, expected_version, version).await
}

pub(crate) async fn update_relational_patch_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    schema: &ReadModelSchema,
    key: &RowKey,
    write_values: Vec<(&ColumnDef, RowValue)>,
    expected_version: u64,
    version: u64,
) -> Result<u64, ReadModelError>
where
    DB: SqlxReadModelBackend,
    for<'c> &'c mut <DB as Database>::Connection: Executor<'c, Database = DB>,
    <DB as Database>::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'r> &'r str: sqlx::ColumnIndex<<DB as Database>::Row>,
{
    update_relational_columns_in_tx(tx, schema, key, write_values, expected_version, version).await
}

pub(crate) async fn update_relational_columns_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    schema: &ReadModelSchema,
    key: &RowKey,
    write_values: Vec<(&ColumnDef, RowValue)>,
    expected_version: u64,
    version: u64,
) -> Result<u64, ReadModelError>
where
    DB: SqlxReadModelBackend,
    for<'c> &'c mut <DB as Database>::Connection: Executor<'c, Database = DB>,
    <DB as Database>::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'r> &'r str: sqlx::ColumnIndex<<DB as Database>::Row>,
{
    let version_column = version_column(schema)?;
    let mut builder = QueryBuilder::<DB>::new("UPDATE ");
    builder.push(quote_identifier(&schema.table_name));
    builder.push(" SET ");
    let mut wrote_set = false;
    for (column, value) in write_values {
        if wrote_set {
            builder.push(", ");
        }
        builder.push(quote_identifier(&column.column_name));
        builder.push(" = ");
        DB::push_row_value_bind(&mut builder, value, column)?;
        wrote_set = true;
    }
    if wrote_set {
        builder.push(", ");
    }
    builder.push(quote_identifier(version_column));
    builder.push(" = ");
    builder.push_bind(read_model_i64_from_u64(
        DB::BACKEND,
        version,
        version_column,
        DB::INTEGER_STORAGE,
    )?);
    push_key_predicates(&mut builder, schema, key)?;
    builder.push(" AND ");
    builder.push(quote_identifier(version_column));
    builder.push(" = ");
    builder.push_bind(read_model_i64_from_u64(
        DB::BACKEND,
        expected_version,
        "expected version",
        DB::INTEGER_STORAGE,
    )?);

    let result = builder
        .build()
        .execute(&mut **tx)
        .await
        .map_err(|err| read_model_storage_error(DB::BACKEND, "update relational row", err))?;
    Ok(DB::rows_affected(&result))
}

pub(crate) async fn delete_relational_row_where_current_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    schema: &ReadModelSchema,
    key: &RowKey,
    current_version: Option<u64>,
) -> Result<u64, ReadModelError>
where
    DB: SqlxReadModelBackend,
    for<'c> &'c mut <DB as Database>::Connection: Executor<'c, Database = DB>,
    <DB as Database>::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'r> &'r str: sqlx::ColumnIndex<<DB as Database>::Row>,
{
    let mut builder = QueryBuilder::<DB>::new("DELETE FROM ");
    builder.push(quote_identifier(&schema.table_name));
    push_key_predicates(&mut builder, schema, key)?;
    if let Some(version) = current_version {
        let version_column = version_column(schema)?;
        builder.push(" AND ");
        builder.push(quote_identifier(version_column));
        builder.push(" = ");
        builder.push_bind(read_model_i64_from_u64(
            DB::BACKEND,
            version,
            "expected version",
            DB::INTEGER_STORAGE,
        )?);
    }

    let result = builder
        .build()
        .execute(&mut **tx)
        .await
        .map_err(|err| read_model_storage_error(DB::BACKEND, "delete relational row", err))?;
    Ok(DB::rows_affected(&result))
}

pub(crate) fn push_key_predicates<DB: SqlxReadModelBackend>(
    builder: &mut QueryBuilder<DB>,
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
        DB::push_row_value_bind(builder, value, column)?;
    }
    Ok(())
}
