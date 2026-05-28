//! InMemoryReadModelStore - HashMap-backed read model store for testing and development.
#![expect(
    clippy::manual_async_fn,
    reason = "async trait impls return impl Future + Send to preserve public Send bounds"
)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, RwLock};

use super::session::{column_name_for, key_fingerprint, validate_key, validate_row_values};
use super::{
    ExpectedVersion, PatchMode, ProcessedMessageMark, ReadModelAdapterCapabilities,
    ReadModelCommitOutcome, ReadModelError, ReadModelIncludeRows, ReadModelLoadGraph,
    ReadModelLoadRequest, ReadModelMutation, ReadModelQueryCapabilities, ReadModelSchema,
    ReadModelSchemaRegistry, ReadModelWritePlan, ReadModelWritePlanStore, RelationalReadModel,
    RelationalReadModelQueryStore, RelationshipDef, RelationshipKind, RowKey, RowValue, RowValues,
    RowWriteMode, Versioned,
};
use crate::repository::{AsyncReadModelWritePlanStore, AsyncRelationalReadModelQueryStore};

#[derive(Clone)]
pub(crate) struct StoredRow {
    pub(crate) values: RowValues,
    pub(crate) version: u64,
}

pub(crate) type ProcessedMessageSet = HashSet<(String, String)>;

pub(crate) const INITIAL_MODEL_VERSION: u64 = 1;

/// Return the next optimistic version for a read model row.
///
/// Missing rows start at version 1; existing rows must increment without
/// overflowing so release builds cannot wrap back to 0.
pub(crate) fn next_model_version(
    key: &str,
    current_version: Option<u64>,
) -> Result<u64, ReadModelError> {
    match current_version {
        Some(version) => version.checked_add(1).ok_or_else(|| {
            ReadModelError::Storage(format!("read model version overflow for {key}"))
        }),
        None => Ok(INITIAL_MODEL_VERSION),
    }
}

fn relational_capabilities() -> ReadModelAdapterCapabilities {
    ReadModelAdapterCapabilities::default()
}

pub(crate) fn apply_read_model_write_plan(
    plan: ReadModelWritePlan,
    staged_rows: &mut HashMap<String, StoredRow>,
    staged_processed_messages: &mut ProcessedMessageSet,
) -> Result<ReadModelCommitOutcome, ReadModelError> {
    plan.validate_for(&relational_capabilities())?;

    let mut marks_in_plan = HashSet::with_capacity(plan.processed_messages.len());
    for mark in &plan.processed_messages {
        let key = processed_message_key(mark);
        if staged_processed_messages.contains(&key) || !marks_in_plan.insert(key) {
            return Ok(ReadModelCommitOutcome::skipped_duplicate(mark.clone()));
        }
    }

    for mutation in plan.mutations {
        match mutation {
            ReadModelMutation::UpsertRow(mutation) => {
                let key = relational_storage_key(&mutation.schema.table_name, &mutation.key);
                let current_version = staged_rows.get(&key).map(|row| row.version);
                validate_row_expected_version(
                    &mutation.schema,
                    &mutation.key,
                    &mutation.expected_version,
                    current_version,
                )?;
                if matches!(mutation.mode, RowWriteMode::Insert) && current_version.is_some() {
                    return Err(concurrency_conflict(
                        &mutation.schema,
                        &mutation.key,
                        0,
                        current_version.unwrap_or_default(),
                    ));
                }

                let new_version = next_model_version(&key, current_version)?;
                staged_rows.insert(
                    key,
                    StoredRow {
                        values: mutation.values,
                        version: new_version,
                    },
                );
            }
            ReadModelMutation::PatchRow(mutation) => {
                let key = relational_storage_key(&mutation.schema.table_name, &mutation.key);
                let current_version = staged_rows.get(&key).map(|row| row.version);
                validate_row_expected_version(
                    &mutation.schema,
                    &mutation.key,
                    &mutation.expected_version,
                    current_version,
                )?;

                match staged_rows.get_mut(&key) {
                    Some(row) => {
                        apply_patch_values_preserving_key(
                            &mutation.schema,
                            &mutation.key,
                            &mut row.values,
                            mutation.patch.into_values(),
                        )?;
                        row.version = next_model_version(&key, current_version)?;
                    }
                    None if matches!(mutation.mode, PatchMode::InsertMissing) => {
                        let values = row_values_from_key_and_patch(
                            &mutation.schema,
                            &mutation.key,
                            mutation.patch.into_values(),
                        )?;
                        staged_rows.insert(
                            key.clone(),
                            StoredRow {
                                values,
                                version: INITIAL_MODEL_VERSION,
                            },
                        );
                    }
                    None => {
                        return Err(ReadModelError::NotFound {
                            table: mutation.schema.table_name,
                            id: key_fingerprint(&mutation.key),
                        });
                    }
                }
            }
            ReadModelMutation::DeleteRow(mutation) => {
                let key = relational_storage_key(&mutation.schema.table_name, &mutation.key);
                let current_version = staged_rows.get(&key).map(|row| row.version);
                validate_row_expected_version(
                    &mutation.schema,
                    &mutation.key,
                    &mutation.expected_version,
                    current_version,
                )?;
                staged_rows.remove(&key);
            }
        }
    }

    for mark in plan.processed_messages {
        staged_processed_messages.insert(processed_message_key(&mark));
    }

    Ok(ReadModelCommitOutcome::applied())
}

fn processed_message_key(mark: &ProcessedMessageMark) -> (String, String) {
    (mark.consumer_name.clone(), mark.message_id.clone())
}

fn relational_storage_key(table_name: &str, key: &RowKey) -> String {
    format!("{}:{}", table_name, key_fingerprint(key))
}

fn row_values_from_key_and_patch(
    schema: &ReadModelSchema,
    key: &RowKey,
    patch_values: RowValues,
) -> Result<RowValues, ReadModelError> {
    let mut values = RowValues::new();
    for (column, value) in key.iter() {
        values.insert(column.to_string(), value.clone());
    }
    apply_patch_values_preserving_key(schema, key, &mut values, patch_values)?;
    validate_row_values(schema, &values, true)?;
    Ok(values)
}

fn apply_patch_values_preserving_key(
    schema: &ReadModelSchema,
    key: &RowKey,
    values: &mut RowValues,
    patch_values: RowValues,
) -> Result<(), ReadModelError> {
    for (column, value) in patch_values {
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
    Ok(())
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
            Err(concurrency_conflict(schema, key, *expected, actual))
        }
        (ExpectedVersion::Exact(_), None) => Err(ReadModelError::NotFound {
            table: schema.table_name.clone(),
            id: key_fingerprint(key),
        }),
        (ExpectedVersion::NotExists, None) => Ok(()),
        (ExpectedVersion::NotExists, Some(actual)) => {
            Err(concurrency_conflict(schema, key, 0, actual))
        }
    }
}

fn concurrency_conflict(
    schema: &ReadModelSchema,
    key: &RowKey,
    expected: u64,
    actual: u64,
) -> ReadModelError {
    ReadModelError::ConcurrencyConflict {
        table: schema.table_name.clone(),
        id: key_fingerprint(key),
        expected,
        actual,
    }
}

/// In-memory read model store backed by a HashMap.
///
/// Clone-friendly via Arc.
#[derive(Clone)]
pub struct InMemoryReadModelStore {
    pub(crate) relational_rows: Arc<RwLock<HashMap<String, StoredRow>>>,
    pub(crate) processed_messages: Arc<RwLock<ProcessedMessageSet>>,
    schema_registry: Arc<RwLock<ReadModelSchemaRegistry>>,
}

impl Default for InMemoryReadModelStore {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryReadModelStore {
    /// Create a new empty read model store.
    pub fn new() -> Self {
        Self {
            relational_rows: Arc::new(RwLock::new(HashMap::new())),
            processed_messages: Arc::new(RwLock::new(HashSet::new())),
            schema_registry: Arc::new(RwLock::new(ReadModelSchemaRegistry::new())),
        }
    }

    /// Register a relational read-model schema for explicit include execution.
    pub fn register_schema<M>(&self) -> Result<(), ReadModelError>
    where
        M: RelationalReadModel,
    {
        let mut registry = self
            .schema_registry
            .write()
            .map_err(|_| ReadModelError::Storage("schema registry lock poisoned".into()))?;
        registry.register::<M>()?;
        Ok(())
    }

    /// Register an already-built relational read-model schema.
    pub fn register_read_model_schema(
        &self,
        schema: ReadModelSchema,
    ) -> Result<(), ReadModelError> {
        let mut registry = self
            .schema_registry
            .write()
            .map_err(|_| ReadModelError::Storage("schema registry lock poisoned".into()))?;
        registry.register_schema(schema)?;
        Ok(())
    }
}

impl ReadModelWritePlanStore for InMemoryReadModelStore {
    fn read_model_capabilities(&self) -> ReadModelAdapterCapabilities {
        relational_capabilities()
    }

    fn commit_write_plan(
        &self,
        plan: ReadModelWritePlan,
    ) -> Result<ReadModelCommitOutcome, ReadModelError> {
        let mut relational_rows = self
            .relational_rows
            .write()
            .map_err(|_| ReadModelError::Storage("lock poisoned".into()))?;
        let mut processed_messages = self
            .processed_messages
            .write()
            .map_err(|_| ReadModelError::Storage("lock poisoned".into()))?;

        let mut staged_rows = relational_rows.clone();
        let mut staged_processed_messages = processed_messages.clone();
        let outcome =
            apply_read_model_write_plan(plan, &mut staged_rows, &mut staged_processed_messages)?;

        if outcome.was_applied() {
            *relational_rows = staged_rows;
            *processed_messages = staged_processed_messages;
        }

        Ok(outcome)
    }

    fn is_processed(&self, consumer_name: &str, message_id: &str) -> Result<bool, ReadModelError> {
        let processed_messages = self
            .processed_messages
            .read()
            .map_err(|_| ReadModelError::Storage("lock poisoned".into()))?;
        Ok(processed_messages.contains(&(consumer_name.to_string(), message_id.to_string())))
    }
}

impl AsyncReadModelWritePlanStore for InMemoryReadModelStore {
    fn read_model_capabilities_async(&self) -> ReadModelAdapterCapabilities {
        ReadModelWritePlanStore::read_model_capabilities(self)
    }

    fn commit_write_plan_async(
        &self,
        plan: ReadModelWritePlan,
    ) -> impl Future<Output = Result<ReadModelCommitOutcome, ReadModelError>> + Send + '_ {
        async move { ReadModelWritePlanStore::commit_write_plan(self, plan) }
    }

    fn is_processed_async<'a>(
        &'a self,
        consumer_name: &'a str,
        message_id: &'a str,
    ) -> impl Future<Output = Result<bool, ReadModelError>> + Send + 'a {
        async move { ReadModelWritePlanStore::is_processed(self, consumer_name, message_id) }
    }
}

#[derive(Clone)]
struct IncludeSpec {
    name: String,
    relationship: RelationshipDef,
    target_schema: ReadModelSchema,
}

impl RelationalReadModelQueryStore for InMemoryReadModelStore {
    fn read_model_query_capabilities(&self) -> ReadModelQueryCapabilities {
        ReadModelQueryCapabilities::relationship_includes()
    }

    fn load_graph(
        &self,
        request: ReadModelLoadRequest,
    ) -> Result<ReadModelLoadGraph, ReadModelError> {
        request.validate_for_query_capabilities(&self.read_model_query_capabilities())?;

        let (root_schema, include_specs) = {
            let registry = self
                .schema_registry
                .read()
                .map_err(|_| ReadModelError::Storage("schema registry lock poisoned".into()))?;
            resolve_request_schemas(&registry, &request)?
        };
        validate_key(&root_schema, &request.key)?;

        let rows = self
            .relational_rows
            .read()
            .map_err(|_| ReadModelError::Storage("lock poisoned".into()))?;
        let root_storage_key = relational_storage_key(&root_schema.table_name, &request.key);
        let Some(root_row) = rows.get(&root_storage_key) else {
            return Ok(ReadModelLoadGraph::default());
        };
        let root = Versioned {
            data: root_row.values.clone(),
            version: root_row.version,
        };

        let mut includes = BTreeMap::new();
        for spec in include_specs {
            let loaded_rows = load_relationship_rows(&rows, &root_schema, &root.data, &spec)?;
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

impl AsyncRelationalReadModelQueryStore for InMemoryReadModelStore {
    fn read_model_query_capabilities_async(&self) -> ReadModelQueryCapabilities {
        RelationalReadModelQueryStore::read_model_query_capabilities(self)
    }

    fn load_graph_async(
        &self,
        request: ReadModelLoadRequest,
    ) -> impl Future<Output = Result<ReadModelLoadGraph, ReadModelError>> + Send + '_ {
        async move { RelationalReadModelQueryStore::load_graph(self, request) }
    }
}

fn resolve_request_schemas(
    registry: &ReadModelSchemaRegistry,
    request: &ReadModelLoadRequest,
) -> Result<(ReadModelSchema, Vec<IncludeSpec>), ReadModelError> {
    let root_schema = registry
        .schema_for_model(&request.schema.model_name)
        .cloned()
        .or_else(|| {
            if request.includes.is_empty() {
                Some(request.schema.clone())
            } else {
                None
            }
        })
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

fn load_relationship_rows(
    rows: &HashMap<String, StoredRow>,
    root_schema: &ReadModelSchema,
    root_row: &RowValues,
    spec: &IncludeSpec,
) -> Result<Vec<Versioned<RowValues>>, ReadModelError> {
    match spec.relationship.kind {
        RelationshipKind::HasMany => load_has_many_rows(rows, root_schema, root_row, spec),
        RelationshipKind::BelongsTo => load_belongs_to_rows(rows, root_schema, root_row, spec),
        RelationshipKind::ManyToMany => Err(ReadModelError::Metadata(format!(
            "many-to-many relationship `{}` includes are not supported yet",
            spec.relationship.field_name
        ))),
    }
}

fn load_has_many_rows(
    rows: &HashMap<String, StoredRow>,
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
    Ok(rows_matching_column(
        rows,
        &spec.target_schema.table_name,
        &target_column,
        root_value,
    ))
}

fn load_belongs_to_rows(
    rows: &HashMap<String, StoredRow>,
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
    let key = RowKey::new([(target_column, source_value.clone())]);
    let storage_key = relational_storage_key(&spec.target_schema.table_name, &key);
    Ok(rows
        .get(&storage_key)
        .map(|row| {
            vec![Versioned {
                data: row.values.clone(),
                version: row.version,
            }]
        })
        .unwrap_or_default())
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

fn rows_matching_column(
    rows: &HashMap<String, StoredRow>,
    table_name: &str,
    column: &str,
    value: &RowValue,
) -> Vec<Versioned<RowValues>> {
    let prefix = format!("{table_name}:");
    let mut matches = rows
        .iter()
        .filter(|(key, row)| key.starts_with(&prefix) && row.values.get(column) == Some(value))
        .map(|(key, row)| {
            (
                key.clone(),
                Versioned {
                    data: row.values.clone(),
                    version: row.version,
                },
            )
        })
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| left.0.cmp(&right.0));
    matches.into_iter().map(|(_, row)| row).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ColumnDef, ColumnType, DeleteRowMutation, PatchRowMutation, PrimaryKey, RowMutation,
        RowPatch,
    };

    fn test_row_schema() -> ReadModelSchema {
        ReadModelSchema {
            model_name: "TestRow".into(),
            table_name: "test_rows".into(),
            columns: vec![ColumnDef::new("id", "id", ColumnType::Text)],
            primary_key: PrimaryKey::new(["id"]),
            version_column: None,
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships: Vec::new(),
        }
    }

    #[test]
    fn relational_write_plan_upserts_rows_and_marks_processed() {
        let store = InMemoryReadModelStore::new();
        let schema = test_row_schema();
        let key = RowKey::new([("id", RowValue::String("row-1".into()))]);
        let mut values = RowValues::new();
        values.insert("id", RowValue::String("row-1".into()));

        let outcome = store
            .commit_write_plan(ReadModelWritePlan::new(
                vec![ReadModelMutation::UpsertRow(RowMutation {
                    schema: schema.clone(),
                    key: key.clone(),
                    values,
                    expected_version: ExpectedVersion::Any,
                    mode: RowWriteMode::Upsert,
                })],
                vec![ProcessedMessageMark {
                    consumer_name: "projection".into(),
                    message_id: "event-1".into(),
                }],
            ))
            .unwrap();
        let row = store
            .relational_rows
            .read()
            .unwrap()
            .get(&relational_storage_key(&schema.table_name, &key))
            .cloned()
            .unwrap();

        assert!(outcome.was_applied());
        assert_eq!(row.version, 1);
        assert_eq!(
            row.values.get("id"),
            Some(&RowValue::String("row-1".into()))
        );
        assert!(store.is_processed("projection", "event-1").unwrap());
    }

    #[test]
    fn relational_write_plan_patches_and_deletes_rows() {
        let store = InMemoryReadModelStore::new();
        let schema = test_row_schema();
        let key = RowKey::new([("id", RowValue::String("row-1".into()))]);
        let mut values = RowValues::new();
        values.insert("id", RowValue::String("row-1".into()));

        store
            .commit_write_plan(ReadModelWritePlan::new(
                vec![ReadModelMutation::UpsertRow(RowMutation {
                    schema: schema.clone(),
                    key: key.clone(),
                    values,
                    expected_version: ExpectedVersion::Any,
                    mode: RowWriteMode::Upsert,
                })],
                Vec::new(),
            ))
            .unwrap();
        store
            .commit_write_plan(ReadModelWritePlan::new(
                vec![ReadModelMutation::PatchRow(PatchRowMutation {
                    schema: schema.clone(),
                    key: key.clone(),
                    patch: RowPatch::new().set("id", RowValue::String("row-1".into())),
                    expected_version: ExpectedVersion::Exact(1),
                    mode: PatchMode::UpdateExisting,
                })],
                Vec::new(),
            ))
            .unwrap();
        let version = store
            .relational_rows
            .read()
            .unwrap()
            .get(&relational_storage_key(&schema.table_name, &key))
            .unwrap()
            .version;
        assert_eq!(version, 2);

        store
            .commit_write_plan(ReadModelWritePlan::new(
                vec![ReadModelMutation::DeleteRow(DeleteRowMutation {
                    schema: schema.clone(),
                    key: key.clone(),
                    expected_version: ExpectedVersion::Exact(2),
                })],
                Vec::new(),
            ))
            .unwrap();
        assert!(!store
            .relational_rows
            .read()
            .unwrap()
            .contains_key(&relational_storage_key(&schema.table_name, &key)));
    }
}
