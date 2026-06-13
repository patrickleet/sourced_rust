//! Backend-agnostic read-model logic shared by the Postgres and SQLite repositories.
//!
//! These functions contain no `sqlx` database types: they validate write plans,
//! reconcile row keys with patches, compute row versions, and resolve relationship
//! includes against the table-schema registry. Both backends produce behaviorally
//! identical results, so the logic lives here once rather than being mirrored (and
//! risking silent drift) in `postgres_repo` and `sqlite_repo`.

use std::sync::RwLock;

use crate::read_model::{
    key_fingerprint, validate_row_values, ColumnDef, ExpectedVersion, ReadModelAdapterCapabilities,
    ReadModelError, ReadModelLoadRequest, ReadModelSchema, ReadModelWritePlan, RelationshipDef,
    RelationshipKind, RowKey, RowValue, RowValues,
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
