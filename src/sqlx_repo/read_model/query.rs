use sqlx::{Database, Encode, QueryBuilder, Row, Type};

use crate::read_model::Versioned;
use crate::sqlx_repo::{read_model_storage_error, read_model_u64_from_i64};
use crate::table::{RowKey, RowValues, TableSchema, TableStoreError};

use super::{column_by_name, quote_identifier, version_column, SqlxReadModelBackend};

pub(crate) fn push_key_predicates<DB: SqlxReadModelBackend>(
    builder: &mut QueryBuilder<DB>,
    schema: &TableSchema,
    key: &RowKey,
) -> Result<(), TableStoreError> {
    builder.push(" WHERE ");
    for (index, column_name) in schema.primary_key.columns.iter().enumerate() {
        if index > 0 {
            builder.push(" AND ");
        }
        let column = column_by_name(schema, column_name)?;
        let value = key.get(column_name).cloned().ok_or_else(|| {
            TableStoreError::Metadata(format!(
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

/// Build the shared `SELECT <columns>, <version> FROM <table>` prefix used by every
/// relational read. Per-column rendering is dialect-specific (`push_select_column`).
pub(crate) fn relational_row_select<DB: SqlxReadModelBackend>(
    schema: &TableSchema,
) -> Result<QueryBuilder<DB>, TableStoreError> {
    let version_column = version_column(schema)?;
    let mut builder = QueryBuilder::<DB>::new("SELECT ");
    for (index, column) in schema.columns.iter().enumerate() {
        if index > 0 {
            builder.push(", ");
        }
        DB::push_select_column(&mut builder, column);
    }
    if !schema.columns.is_empty() {
        builder.push(", ");
    }
    builder.push(quote_identifier(version_column));
    builder.push(" FROM ");
    builder.push(quote_identifier(&schema.table_name));
    Ok(builder)
}

pub(crate) fn push_order_by_primary_key<DB: SqlxReadModelBackend>(
    builder: &mut QueryBuilder<DB>,
    schema: &TableSchema,
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

pub(crate) fn row_to_versioned_values<DB>(
    schema: &TableSchema,
    row: &DB::Row,
) -> Result<Versioned<RowValues>, TableStoreError>
where
    DB: SqlxReadModelBackend,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'r> &'r str: sqlx::ColumnIndex<<DB as Database>::Row>,
{
    let mut values = RowValues::new();
    for column in &schema.columns {
        values.insert(column.column_name.clone(), DB::row_value(row, column)?);
    }
    let version_column = version_column(schema)?;
    let version = read_model_u64_from_i64(
        DB::BACKEND,
        row.try_get::<i64, _>(version_column).map_err(|err| {
            read_model_storage_error(DB::BACKEND, "decode relational row version", err)
        })?,
        version_column,
    )?;
    Ok(Versioned {
        data: values,
        version,
    })
}
