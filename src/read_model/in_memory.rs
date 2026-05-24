//! InMemoryReadModelStore - HashMap-backed read model store for testing and development.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, RwLock};

use super::session::{
    column_name_for, document_key, document_key_prefix, key_fingerprint, validate_key,
    validate_row_values,
};
use super::{
    ExpectedVersion, PatchMode, ProcessedMessageMark, ReadModel, ReadModelAdapterCapabilities,
    ReadModelCommitOutcome, ReadModelError, ReadModelIncludeRows, ReadModelLoadGraph,
    ReadModelLoadRequest, ReadModelMutation, ReadModelQueryCapabilities, ReadModelSchema,
    ReadModelSchemaRegistry, ReadModelSessionStore, ReadModelStore, ReadModelWritePlan,
    RelationalReadModel, RelationalReadModelQueryStore, RelationshipDef, RelationshipKind, RowKey,
    RowValue, RowValues, RowWriteMode, Versioned,
};

/// Internal stored representation of a read model.
#[derive(Clone)]
pub(crate) struct StoredModel {
    pub(crate) bytes: Vec<u8>,
    pub(crate) version: u64,
}

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

pub(crate) fn apply_document_write_plan(
    plan: ReadModelWritePlan,
    staged_models: &mut HashMap<String, StoredModel>,
    staged_processed_messages: &mut ProcessedMessageSet,
) -> Result<ReadModelCommitOutcome, ReadModelError> {
    let capabilities = document_capabilities();
    reject_non_document_mutations(&plan)?;
    plan.validate_for(&capabilities)?;

    let mut marks_in_plan = HashSet::with_capacity(plan.processed_messages.len());
    for mark in &plan.processed_messages {
        let key = processed_message_key(mark);
        if staged_processed_messages.contains(&key) || !marks_in_plan.insert(key) {
            return Ok(ReadModelCommitOutcome::skipped_duplicate(mark.clone()));
        }
    }

    for mutation in plan.mutations {
        if let ReadModelMutation::Document(mutation) = mutation {
            let key = mutation.key();
            let new_version = next_model_version(&key, staged_models.get(&key).map(|s| s.version))?;
            staged_models.insert(
                key,
                StoredModel {
                    bytes: mutation.bytes,
                    version: new_version,
                },
            );
        }
    }

    for mark in plan.processed_messages {
        staged_processed_messages.insert(processed_message_key(&mark));
    }

    Ok(ReadModelCommitOutcome::applied())
}

fn reject_non_document_mutations(plan: &ReadModelWritePlan) -> Result<(), ReadModelError> {
    for mutation in &plan.mutations {
        let mutation_name = match mutation {
            ReadModelMutation::Document(_) => continue,
            ReadModelMutation::UpsertRow(_) => "ReadModelMutation::UpsertRow",
            ReadModelMutation::PatchRow(_) => "ReadModelMutation::PatchRow",
            ReadModelMutation::DeleteRow(_) => "ReadModelMutation::DeleteRow",
        };

        return Err(ReadModelError::Metadata(format!(
            "apply_document_write_plan supports only ReadModelMutation::Document with document_capabilities; received {mutation_name}"
        )));
    }

    Ok(())
}

pub(crate) fn document_capabilities() -> ReadModelAdapterCapabilities {
    ReadModelAdapterCapabilities {
        relational_rows: false,
        document_rows: true,
        sparse_patches: false,
        deletes: false,
        processed_messages: true,
    }
}

fn relational_capabilities() -> ReadModelAdapterCapabilities {
    ReadModelAdapterCapabilities::default()
}

fn apply_read_model_write_plan(
    plan: ReadModelWritePlan,
    staged_models: &mut HashMap<String, StoredModel>,
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
            ReadModelMutation::Document(mutation) => {
                let key = mutation.key();
                let new_version =
                    next_model_version(&key, staged_models.get(&key).map(|s| s.version))?;
                staged_models.insert(
                    key,
                    StoredModel {
                        bytes: mutation.bytes,
                        version: new_version,
                    },
                );
            }
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
                            collection: mutation.schema.table_name,
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
            collection: schema.table_name.clone(),
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
        collection: schema.table_name.clone(),
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
    pub(crate) storage: Arc<RwLock<HashMap<String, StoredModel>>>,
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
            storage: Arc::new(RwLock::new(HashMap::new())),
            relational_rows: Arc::new(RwLock::new(HashMap::new())),
            processed_messages: Arc::new(RwLock::new(HashSet::new())),
            schema_registry: Arc::new(RwLock::new(ReadModelSchemaRegistry::new())),
        }
    }

    fn make_key(table: &str, id: &str) -> String {
        document_key(table, id)
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

    /// Save pre-serialized document bytes by storage key for in-memory test setup.
    #[cfg(test)]
    pub(crate) fn save_document_bytes(
        &self,
        key: &str,
        bytes: Vec<u8>,
    ) -> Result<u64, ReadModelError> {
        let mut storage = self
            .storage
            .write()
            .map_err(|_| ReadModelError::Storage("lock poisoned".into()))?;

        let new_version = next_model_version(key, storage.get(key).map(|s| s.version))?;

        storage.insert(
            key.to_string(),
            StoredModel {
                bytes,
                version: new_version,
            },
        );

        Ok(new_version)
    }
}

impl ReadModelSessionStore for InMemoryReadModelStore {
    fn read_model_capabilities(&self) -> ReadModelAdapterCapabilities {
        relational_capabilities()
    }

    fn commit_write_plan(
        &self,
        plan: ReadModelWritePlan,
    ) -> Result<ReadModelCommitOutcome, ReadModelError> {
        let mut storage = self
            .storage
            .write()
            .map_err(|_| ReadModelError::Storage("lock poisoned".into()))?;
        let mut relational_rows = self
            .relational_rows
            .write()
            .map_err(|_| ReadModelError::Storage("lock poisoned".into()))?;
        let mut processed_messages = self
            .processed_messages
            .write()
            .map_err(|_| ReadModelError::Storage("lock poisoned".into()))?;

        let mut staged_models = storage.clone();
        let mut staged_rows = relational_rows.clone();
        let mut staged_processed_messages = processed_messages.clone();
        let outcome = apply_read_model_write_plan(
            plan,
            &mut staged_models,
            &mut staged_rows,
            &mut staged_processed_messages,
        )?;

        if outcome.was_applied() {
            *storage = staged_models;
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

fn resolve_request_schemas(
    registry: &ReadModelSchemaRegistry,
    request: &ReadModelLoadRequest,
) -> Result<(ReadModelSchema, Vec<IncludeSpec>), ReadModelError> {
    let root_schema = registry
        .schema_for_model(&request.schema.model_name)
        .ok_or_else(|| {
            ReadModelError::Metadata(format!(
                "read model `{}` is not registered for relationship includes",
                request.schema.model_name
            ))
        })?;
    if root_schema != &request.schema {
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

    Ok((root_schema.clone(), include_specs))
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

impl ReadModelStore for InMemoryReadModelStore {
    fn get_model<M: ReadModel>(&self, id: &str) -> Result<Option<Versioned<M>>, ReadModelError> {
        let key = Self::make_key(M::COLLECTION, id);
        let storage = self
            .storage
            .read()
            .map_err(|_| ReadModelError::Storage("lock poisoned".into()))?;

        match storage.get(&key) {
            Some(stored) => {
                let data: M = serde_json::from_slice(&stored.bytes)
                    .map_err(|e| ReadModelError::Serde(e.to_string()))?;
                Ok(Some(Versioned {
                    data,
                    version: stored.version,
                }))
            }
            None => Ok(None),
        }
    }

    fn upsert<M: ReadModel>(&self, model: &M) -> Result<Versioned<M>, ReadModelError> {
        let key = Self::make_key(M::COLLECTION, model.id());
        let bytes = serde_json::to_vec(model).map_err(|e| ReadModelError::Serde(e.to_string()))?;

        let mut storage = self
            .storage
            .write()
            .map_err(|_| ReadModelError::Storage("lock poisoned".into()))?;

        let new_version = next_model_version(&key, storage.get(&key).map(|s| s.version))?;

        storage.insert(
            key,
            StoredModel {
                bytes,
                version: new_version,
            },
        );

        Ok(Versioned {
            data: model.clone(),
            version: new_version,
        })
    }

    fn insert<M: ReadModel>(&self, model: &M) -> Result<Versioned<M>, ReadModelError> {
        let key = Self::make_key(M::COLLECTION, model.id());
        let bytes = serde_json::to_vec(model).map_err(|e| ReadModelError::Serde(e.to_string()))?;

        let mut storage = self
            .storage
            .write()
            .map_err(|_| ReadModelError::Storage("lock poisoned".into()))?;

        if storage.contains_key(&key) {
            return Err(ReadModelError::ConcurrencyConflict {
                collection: M::COLLECTION.to_string(),
                id: model.id().to_string(),
                expected: 0,
                actual: storage[&key].version,
            });
        }

        storage.insert(
            key,
            StoredModel {
                bytes,
                version: INITIAL_MODEL_VERSION,
            },
        );

        Ok(Versioned {
            data: model.clone(),
            version: INITIAL_MODEL_VERSION,
        })
    }

    fn update<M: ReadModel>(
        &self,
        model: &M,
        expected_version: u64,
    ) -> Result<Versioned<M>, ReadModelError> {
        let key = Self::make_key(M::COLLECTION, model.id());
        let bytes = serde_json::to_vec(model).map_err(|e| ReadModelError::Serde(e.to_string()))?;

        let mut storage = self
            .storage
            .write()
            .map_err(|_| ReadModelError::Storage("lock poisoned".into()))?;

        let actual_version =
            storage
                .get(&key)
                .map(|s| s.version)
                .ok_or_else(|| ReadModelError::NotFound {
                    collection: M::COLLECTION.to_string(),
                    id: model.id().to_string(),
                })?;

        if actual_version != expected_version {
            return Err(ReadModelError::ConcurrencyConflict {
                collection: M::COLLECTION.to_string(),
                id: model.id().to_string(),
                expected: expected_version,
                actual: actual_version,
            });
        }

        let new_version = next_model_version(&key, Some(actual_version))?;
        storage.insert(
            key,
            StoredModel {
                bytes,
                version: new_version,
            },
        );

        Ok(Versioned {
            data: model.clone(),
            version: new_version,
        })
    }

    fn delete<M: ReadModel>(&self, id: &str) -> Result<bool, ReadModelError> {
        let key = Self::make_key(M::COLLECTION, id);
        let mut storage = self
            .storage
            .write()
            .map_err(|_| ReadModelError::Storage("lock poisoned".into()))?;

        Ok(storage.remove(&key).is_some())
    }

    fn find_models<M: ReadModel>(
        &self,
        predicate: &dyn Fn(&M) -> bool,
    ) -> Result<Vec<Versioned<M>>, ReadModelError> {
        let storage = self
            .storage
            .read()
            .map_err(|_| ReadModelError::Storage("lock poisoned".into()))?;

        let prefix = document_key_prefix(M::COLLECTION);
        let mut results = Vec::new();

        for (key, stored) in storage.iter() {
            if key.starts_with(&prefix) {
                let data = serde_json::from_slice::<M>(&stored.bytes)
                    .map_err(|e| ReadModelError::Serde(e.to_string()))?;
                if predicate(&data) {
                    results.push(Versioned {
                        data,
                        version: stored.version,
                    });
                }
            }
        }

        Ok(results)
    }

    fn find_one_model<M: ReadModel>(
        &self,
        predicate: &dyn Fn(&M) -> bool,
    ) -> Result<Option<Versioned<M>>, ReadModelError> {
        let storage = self
            .storage
            .read()
            .map_err(|_| ReadModelError::Storage("lock poisoned".into()))?;

        let prefix = document_key_prefix(M::COLLECTION);
        let mut matched = None;

        for (key, stored) in storage.iter() {
            if key.starts_with(&prefix) {
                let data = serde_json::from_slice::<M>(&stored.bytes)
                    .map_err(|e| ReadModelError::Serde(e.to_string()))?;
                if matched.is_none() && predicate(&data) {
                    matched = Some(Versioned {
                        data,
                        version: stored.version,
                    });
                }
            }
        }

        Ok(matched)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ColumnDef, ColumnType, PrimaryKey, RowMutation};
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    struct TestModel {
        id: String,
        value: i32,
    }

    impl ReadModel for TestModel {
        const COLLECTION: &'static str = "test_models";
        fn id(&self) -> &str {
            &self.id
        }
    }

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
    fn apply_document_write_plan_rejects_non_document_mutations() {
        let key = RowKey::new([("id", RowValue::String("row-1".into()))]);
        let mut values = RowValues::new();
        values.insert("id", RowValue::String("row-1".into()));
        let plan = ReadModelWritePlan::new(
            vec![ReadModelMutation::UpsertRow(RowMutation {
                schema: test_row_schema(),
                key,
                values,
                expected_version: ExpectedVersion::Any,
                mode: RowWriteMode::Upsert,
            })],
            Vec::new(),
        );
        let mut staged_models = HashMap::new();
        let mut staged_processed_messages = HashSet::new();

        let err =
            apply_document_write_plan(plan, &mut staged_models, &mut staged_processed_messages)
                .unwrap_err();

        assert!(matches!(err, ReadModelError::Metadata(message)
                if message.contains("apply_document_write_plan")
                    && message.contains("ReadModelMutation::UpsertRow")
                    && message.contains("document_capabilities")));
    }

    #[test]
    fn upsert_and_get() {
        let store = InMemoryReadModelStore::new();
        let model = TestModel {
            id: "1".into(),
            value: 42,
        };

        let saved = store.upsert(&model).unwrap();
        assert_eq!(saved.version, 1);
        assert_eq!(saved.data.value, 42);

        let loaded = store.get_model::<TestModel>("1").unwrap().unwrap();
        assert_eq!(loaded.version, 1);
        assert_eq!(loaded.data.value, 42);
    }

    #[test]
    fn upsert_increments_version() {
        let store = InMemoryReadModelStore::new();
        let model = TestModel {
            id: "1".into(),
            value: 1,
        };

        store.upsert(&model).unwrap();
        let updated = TestModel {
            id: "1".into(),
            value: 2,
        };
        let saved = store.upsert(&updated).unwrap();
        assert_eq!(saved.version, 2);
    }

    #[test]
    fn save_document_bytes_returns_error_on_version_overflow() {
        let store = InMemoryReadModelStore::new();
        let key = InMemoryReadModelStore::make_key(TestModel::COLLECTION, "1");
        let bytes = serde_json::to_vec(&TestModel {
            id: "1".into(),
            value: 1,
        })
        .unwrap();
        store.storage.write().unwrap().insert(
            key.clone(),
            StoredModel {
                bytes,
                version: u64::MAX,
            },
        );

        let err = store.save_document_bytes(&key, b"{}".to_vec()).unwrap_err();

        assert!(
            matches!(err, ReadModelError::Storage(message) if message.contains("version overflow"))
        );
    }

    #[test]
    fn upsert_returns_error_on_version_overflow() {
        let store = InMemoryReadModelStore::new();
        let model = TestModel {
            id: "1".into(),
            value: 1,
        };
        let key = InMemoryReadModelStore::make_key(TestModel::COLLECTION, model.id());
        let bytes = serde_json::to_vec(&model).unwrap();
        store.storage.write().unwrap().insert(
            key,
            StoredModel {
                bytes,
                version: u64::MAX,
            },
        );

        let err = store.upsert(&model).unwrap_err();

        assert!(
            matches!(err, ReadModelError::Storage(message) if message.contains("version overflow"))
        );
    }

    #[test]
    fn get_missing_returns_none() {
        let store = InMemoryReadModelStore::new();
        let result = store.get_model::<TestModel>("missing").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn insert_fails_on_existing() {
        let store = InMemoryReadModelStore::new();
        let model = TestModel {
            id: "1".into(),
            value: 1,
        };

        store.insert(&model).unwrap();
        let err = store.insert(&model).unwrap_err();
        assert!(matches!(err, ReadModelError::ConcurrencyConflict { .. }));
    }

    #[test]
    fn update_with_correct_version() {
        let store = InMemoryReadModelStore::new();
        let model = TestModel {
            id: "1".into(),
            value: 1,
        };

        store.upsert(&model).unwrap();

        let updated = TestModel {
            id: "1".into(),
            value: 2,
        };
        let result = store.update(&updated, 1).unwrap();
        assert_eq!(result.version, 2);
        assert_eq!(result.data.value, 2);
    }

    #[test]
    fn update_returns_error_on_version_overflow() {
        let store = InMemoryReadModelStore::new();
        let model = TestModel {
            id: "1".into(),
            value: 1,
        };
        let key = InMemoryReadModelStore::make_key(TestModel::COLLECTION, model.id());
        let bytes = serde_json::to_vec(&model).unwrap();
        store.storage.write().unwrap().insert(
            key,
            StoredModel {
                bytes,
                version: u64::MAX,
            },
        );

        let err = store.update(&model, u64::MAX).unwrap_err();

        assert!(
            matches!(err, ReadModelError::Storage(message) if message.contains("version overflow"))
        );
    }

    #[test]
    fn update_with_wrong_version_fails() {
        let store = InMemoryReadModelStore::new();
        let model = TestModel {
            id: "1".into(),
            value: 1,
        };

        store.upsert(&model).unwrap();

        let updated = TestModel {
            id: "1".into(),
            value: 2,
        };
        let err = store.update(&updated, 99).unwrap_err();
        assert!(matches!(err, ReadModelError::ConcurrencyConflict { .. }));
    }

    #[test]
    fn delete_existing() {
        let store = InMemoryReadModelStore::new();
        let model = TestModel {
            id: "1".into(),
            value: 1,
        };

        store.upsert(&model).unwrap();
        assert!(store.delete::<TestModel>("1").unwrap());
        assert!(store.get_model::<TestModel>("1").unwrap().is_none());
    }

    #[test]
    fn delete_missing_returns_false() {
        let store = InMemoryReadModelStore::new();
        assert!(!store.delete::<TestModel>("missing").unwrap());
    }

    #[test]
    fn find_with_predicate() {
        let store = InMemoryReadModelStore::new();

        store
            .upsert(&TestModel {
                id: "1".into(),
                value: 10,
            })
            .unwrap();
        store
            .upsert(&TestModel {
                id: "2".into(),
                value: 20,
            })
            .unwrap();
        store
            .upsert(&TestModel {
                id: "3".into(),
                value: 5,
            })
            .unwrap();

        let results = store.find_models::<TestModel>(&|m| m.value > 8).unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn find_one_with_predicate() {
        let store = InMemoryReadModelStore::new();

        store
            .upsert(&TestModel {
                id: "1".into(),
                value: 10,
            })
            .unwrap();
        store
            .upsert(&TestModel {
                id: "2".into(),
                value: 20,
            })
            .unwrap();

        let result = store
            .find_one_model::<TestModel>(&|m| m.value > 15)
            .unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().data.value, 20);

        let none = store
            .find_one_model::<TestModel>(&|m| m.value > 100)
            .unwrap();
        assert!(none.is_none());
    }

    #[test]
    fn find_models_returns_error_for_corrupted_rows() {
        let store = InMemoryReadModelStore::new();
        let key = InMemoryReadModelStore::make_key(TestModel::COLLECTION, "bad");
        store
            .save_document_bytes(&key, b"not valid json".to_vec())
            .unwrap();

        let err = store.find_models::<TestModel>(&|_| true).unwrap_err();

        assert!(matches!(err, ReadModelError::Serde(_)));
    }

    #[test]
    fn find_one_model_returns_error_for_corrupted_rows() {
        let store = InMemoryReadModelStore::new();
        let key = InMemoryReadModelStore::make_key(TestModel::COLLECTION, "bad");
        store
            .save_document_bytes(&key, b"not valid json".to_vec())
            .unwrap();

        let err = store.find_one_model::<TestModel>(&|_| true).unwrap_err();

        assert!(matches!(err, ReadModelError::Serde(_)));
    }

    #[test]
    fn find_one_model_validates_rows_after_first_match() {
        let store = InMemoryReadModelStore::new();
        store
            .upsert(&TestModel {
                id: "1".into(),
                value: 20,
            })
            .unwrap();
        let key = InMemoryReadModelStore::make_key(TestModel::COLLECTION, "bad");
        store
            .save_document_bytes(&key, b"not valid json".to_vec())
            .unwrap();

        let err = store
            .find_one_model::<TestModel>(&|m| m.value > 15)
            .unwrap_err();

        assert!(matches!(err, ReadModelError::Serde(_)));
    }

    #[test]
    fn clone_shares_storage() {
        let store = InMemoryReadModelStore::new();
        let clone = store.clone();

        store
            .upsert(&TestModel {
                id: "1".into(),
                value: 42,
            })
            .unwrap();

        let loaded = clone.get_model::<TestModel>("1").unwrap().unwrap();
        assert_eq!(loaded.data.value, 42);
    }
}
