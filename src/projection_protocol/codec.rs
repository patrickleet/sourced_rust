//! Canonical projection partition and record-key encoding.
//!
//! A projector topology owns one codec registry. Command obligations and
//! projector-side row keys both pass through this registry, so field/column
//! aliases and scalar representations cannot silently produce different
//! record identities.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use sha2::{Digest, Sha256};

use crate::table::{ColumnType, RowKey, RowValue, TableColumn, TableKind, TableSchema};

use super::{
    ProjectionModelOwnership, ProjectionPartition, ProjectionProtocolError,
    ProjectionProtocolValidationError, ProjectionRecordScope, ProjectorTopologyId,
    ResolvedProjectionKey, ResolvedProjectionObligation, MAX_PROJECTION_MODEL_NAME_BYTES,
    MAX_PROJECTION_PARTITION_BYTES, MAX_PROJECTION_RECORD_KEY_BYTES,
};

const PARTITION_ENCODING_DOMAIN: &[u8] = b"distributed.projection.scope-partition.v1\0";
const RECORD_KEY_ENCODING_DOMAIN: &[u8] = b"distributed.projection.scope-record-key.v1\0";
const COMPILED_TOPOLOGY_DOMAIN: &[u8] = b"distributed.projection.compiled-topology.v1\0";
const COMPILED_TOPOLOGY_VERSION: u32 = 1;
const SCOPE_CODEC_VERSION: u32 = 1;
const MAX_PARTITION_PATH_DEPTH: usize = 32;
const MAX_PARTITION_PATH_SEGMENT_BYTES: usize = 255;

/// Canonical declaration-owned partition derivation for one projector.
///
/// The runtime evaluates this closed IR from raw JSON before decoding the
/// typed event. It is also part of the compiled topology digest, so changing
/// partition semantics necessarily creates a different durable topology.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ProjectionPartitionSpec {
    Unit,
    InputPath { path: Vec<String> },
    Constant { value: serde_json::Value },
}

impl ProjectionPartitionSpec {
    pub(crate) fn unit() -> Self {
        Self::Unit
    }

    pub(crate) fn input_path(path: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self::InputPath {
            path: path.into_iter().map(Into::into).collect(),
        }
    }

    pub(crate) fn constant(value: serde_json::Value) -> Self {
        Self::Constant { value }
    }

    pub(crate) fn preserves_source_sequence(&self) -> bool {
        matches!(self, Self::Unit | Self::Constant { .. })
    }

    pub(crate) fn requires_input(&self) -> bool {
        matches!(self, Self::InputPath { .. })
    }

    pub(crate) fn validate(&self) -> Result<(), ProjectionProtocolError> {
        match self {
            Self::InputPath { path } => {
                if path.is_empty()
                    || path.len() > MAX_PARTITION_PATH_DEPTH
                    || path.iter().any(|segment| {
                        segment.trim().is_empty()
                            || segment.as_bytes().len() > MAX_PARTITION_PATH_SEGMENT_BYTES
                    })
                {
                    return Err(ProjectionProtocolError::InvalidBatch(format!(
                        "projection partition input path must contain 1..={MAX_PARTITION_PATH_DEPTH} non-empty segments of at most {MAX_PARTITION_PATH_SEGMENT_BYTES} bytes"
                    )));
                }
            }
            Self::Constant { value } => {
                let bytes = serde_json::to_vec(value).map_err(|error| {
                    ProjectionProtocolError::InvalidBatch(format!(
                        "projection partition constant cannot be serialized: {error}"
                    ))
                })?;
                if bytes.len() > MAX_PROJECTION_PARTITION_BYTES {
                    return Err(ProjectionProtocolError::InvalidBatch(format!(
                        "projection partition constant exceeds {MAX_PROJECTION_PARTITION_BYTES} canonical JSON bytes"
                    )));
                }
            }
            Self::Unit => {}
        }
        Ok(())
    }

    pub(crate) fn resolve(
        &self,
        canonical_input: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, ProjectionProtocolError> {
        match self {
            Self::Unit => Ok(None),
            Self::Constant { value } => Ok(Some(value.clone())),
            Self::InputPath { path } => {
                let mut value = canonical_input;
                for segment in path {
                    value = value
                        .as_object()
                        .and_then(|object| object.get(segment))
                        .ok_or_else(|| {
                            ProjectionProtocolError::InvalidBatch(format!(
                                "projection partition input path `{}` is absent",
                                path.join(".")
                            ))
                        })?;
                }
                Ok(Some(value.clone()))
            }
        }
    }
}

/// One compiler-owned projector identity, codec, and complete physical
/// ownership inventory.
///
/// GraphQL direct projection binding and the asynchronous projector runtime
/// both derive their protocol identity through [`compile_projection_topology`].
/// This product additionally retains the full typed schema registry needed by
/// a running asynchronous projector. Application code never receives the
/// codec or the authority to mint protocol scopes.
#[derive(Clone, Debug)]
pub(crate) struct CompiledProjectionTopology {
    topology: ProjectorTopologyId,
    codec: Arc<ProjectionScopeCodec>,
    ownership: Vec<ProjectionModelOwnership>,
    partition: ProjectionPartitionSpec,
}

impl CompiledProjectionTopology {
    pub(crate) fn compile<'a>(
        name: &str,
        facts: &[String],
        declared_models: &[String],
        partition: &ProjectionPartitionSpec,
        schemas: impl IntoIterator<Item = &'a TableSchema>,
    ) -> Result<Self, ProjectionProtocolError> {
        let schemas = schemas.into_iter().collect::<Vec<_>>();
        let (topology, ownership) = compile_projection_topology(
            name,
            facts,
            declared_models,
            partition,
            schemas.iter().copied(),
        )?;
        let codec = ProjectionScopeCodec::with_models(
            topology.clone(),
            schemas
                .iter()
                .map(|schema| (schema.model_name.as_str(), *schema)),
        )
        .map_err(|error| {
            ProjectionProtocolError::InvalidBatch(format!(
                "invalid compiled projection scope codec: {error}"
            ))
        })?;
        Ok(Self {
            topology,
            codec: Arc::new(codec),
            ownership,
            partition: partition.clone(),
        })
    }

    pub(crate) fn topology(&self) -> &ProjectorTopologyId {
        &self.topology
    }

    pub(crate) fn codec(&self) -> Arc<ProjectionScopeCodec> {
        Arc::clone(&self.codec)
    }

    pub(crate) fn ownership(&self) -> &[ProjectionModelOwnership] {
        &self.ownership
    }

    pub(crate) fn partition(&self) -> &ProjectionPartitionSpec {
        &self.partition
    }
}

/// Compile the exact protocol identity and model/table ownership for a
/// projector declaration.
///
/// The digest includes accepted fact names, a fixed version of the canonical
/// partition/key codec, and each complete registered table schema. It therefore
/// changes whenever a model's physical table, field/column mapping, primary-key
/// scope, or other schema contract changes. Callers may supply schemas in any
/// order; the compiler sorts facts and models and rejects duplicates.
pub(crate) fn compile_projection_topology<'a>(
    name: &str,
    facts: &[String],
    declared_models: &[String],
    partition: &ProjectionPartitionSpec,
    schemas: impl IntoIterator<Item = &'a TableSchema>,
) -> Result<(ProjectorTopologyId, Vec<ProjectionModelOwnership>), ProjectionProtocolError> {
    partition.validate()?;
    if facts.is_empty() {
        return Err(ProjectionProtocolError::InvalidBatch(format!(
            "projector `{name}` must declare at least one accepted fact"
        )));
    }
    let mut facts = facts.to_vec();
    facts.sort();
    if facts.iter().any(|fact| fact.trim().is_empty()) {
        return Err(ProjectionProtocolError::InvalidBatch(format!(
            "projector `{name}` contains an empty accepted fact"
        )));
    }
    if facts.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(ProjectionProtocolError::InvalidBatch(format!(
            "projector `{name}` repeats an accepted fact"
        )));
    }

    if declared_models.is_empty() {
        return Err(ProjectionProtocolError::InvalidBatch(format!(
            "projector `{name}` must declare at least one output model"
        )));
    }
    let mut declared_models = declared_models.to_vec();
    declared_models.sort();
    if declared_models.iter().any(|model| model.trim().is_empty()) {
        return Err(ProjectionProtocolError::InvalidBatch(format!(
            "projector `{name}` contains an empty output model"
        )));
    }
    if declared_models.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(ProjectionProtocolError::InvalidBatch(format!(
            "projector `{name}` repeats an output model"
        )));
    }

    let mut schemas_by_model = BTreeMap::new();
    for schema in schemas {
        schema.validate()?;
        if !schema.kind.is_read_model() {
            return Err(ProjectionProtocolError::InvalidBatch(format!(
                "projector `{name}` model `{}` is not a read model",
                schema.model_name
            )));
        }
        if schemas_by_model
            .insert(schema.model_name.clone(), schema)
            .is_some()
        {
            return Err(ProjectionProtocolError::InvalidBatch(format!(
                "projector `{name}` repeats schema `{}`",
                schema.model_name
            )));
        }
    }
    let registered_models = schemas_by_model.keys().cloned().collect::<Vec<_>>();
    if registered_models != declared_models {
        return Err(ProjectionProtocolError::InvalidBatch(format!(
            "projector `{name}` declared models {:?} but registered typed schemas {:?}",
            declared_models, registered_models
        )));
    }

    let mut ownership = Vec::with_capacity(schemas_by_model.len());
    let mut compiled_models = Vec::with_capacity(schemas_by_model.len());
    let mut physical_tables = BTreeSet::new();
    for (model, schema) in schemas_by_model {
        if !physical_tables.insert(schema.table_name.as_str()) {
            return Err(ProjectionProtocolError::InvalidBatch(format!(
                "projector `{name}` assigns more than one model to physical table `{}`",
                schema.table_name
            )));
        }
        ownership.push(ProjectionModelOwnership::new(
            model.clone(),
            schema.table_name.clone(),
        )?);
        compiled_models.push(serde_json::json!({
            "model": model,
            "table": schema.table_name,
            "schema": schema,
        }));
    }

    let canonical = serde_json::json!({
        "topology_version": COMPILED_TOPOLOGY_VERSION,
        "scope_codec_version": SCOPE_CODEC_VERSION,
        "name": name,
        "partition": partition,
        "facts": facts,
        "models": compiled_models,
    });
    let mut digest = Sha256::new();
    digest.update(COMPILED_TOPOLOGY_DOMAIN);
    digest.update(
        serde_json::to_vec(&canonical)
            .expect("compiled projection topology contains only serializable schema metadata"),
    );
    let topology =
        ProjectorTopologyId::new(COMPILED_TOPOLOGY_VERSION, name, digest.finalize().into())?;
    Ok((topology, ownership))
}

/// A topology-bound registry for canonical projection partitions and keys.
///
/// Registration is deliberately explicit: the projector's declared model name
/// must exactly match the registered schema's model name. Obligation keys use
/// Rust/GraphQL `field_name`s, while projector [`RowKey`] values use storage
/// `column_name`s; both are lowered in the schema's primary-key column order.
#[derive(Clone, Debug)]
pub(crate) struct ProjectionScopeCodec {
    topology: ProjectorTopologyId,
    models: BTreeMap<String, Arc<TableSchema>>,
}

impl ProjectionScopeCodec {
    pub(crate) fn new(topology: ProjectorTopologyId) -> Self {
        Self {
            topology,
            models: BTreeMap::new(),
        }
    }

    pub(crate) fn with_models<'a>(
        topology: ProjectorTopologyId,
        models: impl IntoIterator<Item = (&'a str, &'a TableSchema)>,
    ) -> Result<Self, ProjectionScopeCodecError> {
        let mut codec = Self::new(topology);
        for (model, schema) in models {
            codec.register_model(model, schema)?;
        }
        Ok(codec)
    }

    pub(crate) fn topology(&self) -> &ProjectorTopologyId {
        &self.topology
    }

    /// Register one projection model under its compiler-declared model name.
    ///
    /// The codec owns an immutable clone so runtime-loaded manifests and
    /// generated static schemas have identical lifetime and mutation
    /// semantics. A caller may discard or mutate its original clone after
    /// registration without changing the compiled topology.
    pub(crate) fn register_model(
        &mut self,
        declared_model: &str,
        schema: &TableSchema,
    ) -> Result<&mut Self, ProjectionScopeCodecError> {
        validate_registration_name(declared_model)?;
        if declared_model != schema.model_name {
            return Err(ProjectionScopeCodecError::ModelRegistrationMismatch {
                declared: declared_model.to_string(),
                schema: schema.model_name.clone(),
            });
        }
        if self.models.contains_key(declared_model) {
            return Err(ProjectionScopeCodecError::DuplicateModelRegistration {
                model: declared_model.to_string(),
            });
        }
        validate_model_schema(schema)?;
        self.models
            .insert(declared_model.to_string(), Arc::new(schema.clone()));
        Ok(self)
    }

    /// Encode the declaration's optional partition.
    ///
    /// `None` is a canonical unit scope. `Some(Value::Null)` is an explicit
    /// JSON null and therefore has a distinct byte representation and digest.
    pub(crate) fn encode_partition(
        &self,
        partition: Option<&serde_json::Value>,
    ) -> Result<ProjectionPartition, ProjectionScopeCodecError> {
        let mut encoder = CanonicalEncoder::new(
            "projection partition",
            PARTITION_ENCODING_DOMAIN,
            MAX_PROJECTION_PARTITION_BYTES,
        )?;
        match partition {
            None => encoder.push_tag(0)?,
            Some(partition) => {
                encoder.push_tag(1)?;
                encode_json(&mut encoder, partition)?;
            }
        }
        ProjectionPartition::new(encoder.finish()).map_err(Into::into)
    }

    /// Lower a command-ledger obligation into its exact durable record scope.
    pub(crate) fn encode_obligation_scope(
        &self,
        obligation: &ResolvedProjectionObligation,
    ) -> Result<ProjectionRecordScope, ProjectionScopeCodecError> {
        let computed = self.encode_resolved_obligation_scope(
            &obligation.projector,
            &obligation.model,
            &obligation.key,
            obligation.partition.as_ref(),
        )?;
        if computed != obligation.scope {
            return Err(ProjectionScopeCodecError::StoredScopeMismatch {
                projector: obligation.projector.clone(),
                model: obligation.model.clone(),
            });
        }
        Ok(computed)
    }

    pub(crate) fn encode_resolved_obligation_scope(
        &self,
        projector: &str,
        model: &str,
        key: &ResolvedProjectionKey,
        partition_value: Option<&serde_json::Value>,
    ) -> Result<ProjectionRecordScope, ProjectionScopeCodecError> {
        self.validate_projector(projector)?;
        let schema = self.model(model)?;
        let partition = self.encode_partition(partition_value)?;

        let mut fields = BTreeMap::new();
        for field in &key.fields {
            if fields.insert(field.field.as_str(), &field.value).is_some() {
                return Err(ProjectionScopeCodecError::DuplicateKeyField {
                    model: model.to_string(),
                    field: field.field.clone(),
                });
            }
        }

        let primary_key_fields = schema
            .primary_key
            .columns
            .iter()
            .map(|column_name| {
                key_column(schema, column_name)
                    .expect("registered projection schemas retain their validated key columns")
                    .field_name
                    .as_str()
            })
            .collect::<BTreeSet<_>>();
        if let Some(extra) = fields
            .keys()
            .find(|field| !primary_key_fields.contains(**field))
        {
            return Err(ProjectionScopeCodecError::ExtraKeyField {
                model: model.to_string(),
                field: (*extra).to_string(),
            });
        }

        let canonical_key_bytes = self.encode_key(schema, |column| {
            fields
                .get(column.field_name.as_str())
                .copied()
                .ok_or_else(|| ProjectionScopeCodecError::MissingKeyField {
                    model: schema.model_name.clone(),
                    field: column.field_name.clone(),
                })
                .and_then(|value| typed_value_from_json(schema, column, value))
        })?;

        ProjectionRecordScope::new(
            self.topology.clone(),
            partition,
            schema.model_name.clone(),
            canonical_key_bytes,
        )
        .map_err(Into::into)
    }

    /// Lower a projector-side row key into the same durable record scope used
    /// by command obligations.
    pub(crate) fn encode_row_scope(
        &self,
        projector: &str,
        model: &str,
        partition: Option<&serde_json::Value>,
        key: &RowKey,
    ) -> Result<ProjectionRecordScope, ProjectionScopeCodecError> {
        self.validate_projector(projector)?;
        let partition = self.encode_partition(partition)?;
        self.encode_row_scope_in_partition(model, partition, key)
    }

    /// Lower a row key after the framework has already resolved the exact
    /// canonical projector partition.
    ///
    /// Query snapshots use this seam so the physical row key and durable
    /// record scope are derived by the same registered codec. Callers cannot
    /// pair arbitrary row values with independently supplied scope bytes.
    pub(crate) fn encode_row_scope_in_partition(
        &self,
        model: &str,
        partition: ProjectionPartition,
        key: &RowKey,
    ) -> Result<ProjectionRecordScope, ProjectionScopeCodecError> {
        let schema = self.model(model)?;
        let canonical_key_bytes = self.encode_row_key(schema, key)?;

        ProjectionRecordScope::new(
            self.topology.clone(),
            partition,
            schema.model_name.clone(),
            canonical_key_bytes,
        )
        .map_err(Into::into)
    }

    /// Encode only the typed primary-key identity, without choosing a
    /// projector partition.
    ///
    /// Physical query rows do not carry their hidden causal partition. Query
    /// evidence uses these canonical bytes to find the one live record across
    /// partitions, then returns that record's exact stored scope.
    pub(crate) fn encode_unpartitioned_row_key(
        &self,
        model: &str,
        key: &RowKey,
    ) -> Result<Vec<u8>, ProjectionScopeCodecError> {
        let schema = self.model(model)?;
        self.encode_row_key(schema, key)
    }

    /// Decode complete primary-key columns from GraphQL/JSON without passing
    /// through JavaScript numeric coercion or a second schema interpretation.
    ///
    /// Integer keys accept either an exact JSON integer or the canonical
    /// decimal string used by GraphQL `BigInt`. Byte keys require canonical
    /// standard-base64. Returned [`RowKey`] values use physical column names,
    /// ready for [`Self::encode_unpartitioned_row_key`].
    pub(crate) fn row_key_from_json_columns(
        &self,
        model: &str,
        values: &BTreeMap<String, serde_json::Value>,
    ) -> Result<RowKey, ProjectionScopeCodecError> {
        let schema = self.model(model)?;
        let primary_key_columns = schema
            .primary_key
            .columns
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        if let Some(extra) = values
            .keys()
            .find(|column| !primary_key_columns.contains(column.as_str()))
        {
            return Err(ProjectionScopeCodecError::ExtraKeyColumn {
                model: schema.model_name.clone(),
                column: extra.clone(),
            });
        }

        let mut key = RowKey::default();
        for column_name in &schema.primary_key.columns {
            let column = key_column(schema, column_name)
                .expect("registered projection schemas retain their validated key columns");
            let value = values.get(column_name).ok_or_else(|| {
                ProjectionScopeCodecError::MissingKeyColumn {
                    model: schema.model_name.clone(),
                    column: column_name.clone(),
                }
            })?;
            key.insert(
                column_name,
                row_value_from_graphql_json(schema, column, value)?,
            );
        }
        Ok(key)
    }

    fn encode_row_key(
        &self,
        schema: &TableSchema,
        key: &RowKey,
    ) -> Result<Vec<u8>, ProjectionScopeCodecError> {
        let primary_key_columns = schema
            .primary_key
            .columns
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        if let Some((extra, _)) = key
            .iter()
            .find(|(column, _)| !primary_key_columns.contains(*column))
        {
            return Err(ProjectionScopeCodecError::ExtraKeyColumn {
                model: schema.model_name.clone(),
                column: extra.to_string(),
            });
        }

        self.encode_key(schema, |column| {
            key.get(&column.column_name)
                .ok_or_else(|| ProjectionScopeCodecError::MissingKeyColumn {
                    model: schema.model_name.clone(),
                    column: column.column_name.clone(),
                })
                .and_then(|value| typed_value_from_row(schema, column, value))
        })
    }

    pub(crate) fn registered_schema(
        &self,
        model: &str,
    ) -> Result<&TableSchema, ProjectionScopeCodecError> {
        self.model(model)
    }

    pub(crate) fn registered_schema_owned(
        &self,
        model: &str,
    ) -> Result<Arc<TableSchema>, ProjectionScopeCodecError> {
        self.models
            .get(model)
            .cloned()
            .ok_or_else(|| ProjectionScopeCodecError::UnknownModel {
                projector: self.topology.name().to_string(),
                model: model.to_string(),
            })
    }

    fn validate_projector(&self, projector: &str) -> Result<(), ProjectionScopeCodecError> {
        if projector == self.topology.name() {
            Ok(())
        } else {
            Err(ProjectionScopeCodecError::ProjectorMismatch {
                expected: self.topology.name().to_string(),
                actual: projector.to_string(),
            })
        }
    }

    fn model(&self, model: &str) -> Result<&TableSchema, ProjectionScopeCodecError> {
        self.models.get(model).map(AsRef::as_ref).ok_or_else(|| {
            ProjectionScopeCodecError::UnknownModel {
                projector: self.topology.name().to_string(),
                model: model.to_string(),
            }
        })
    }

    fn encode_key(
        &self,
        schema: &TableSchema,
        mut value_for: impl FnMut(&TableColumn) -> Result<TypedKeyValue, ProjectionScopeCodecError>,
    ) -> Result<Vec<u8>, ProjectionScopeCodecError> {
        let mut encoder = CanonicalEncoder::new(
            "projection record key",
            RECORD_KEY_ENCODING_DOMAIN,
            MAX_PROJECTION_RECORD_KEY_BYTES,
        )?;
        encoder.push_len(schema.primary_key.columns.len())?;
        for column_name in &schema.primary_key.columns {
            let column = key_column(schema, column_name)
                .expect("registered projection schemas retain their validated key columns");
            encoder.push_bytes(column.column_name.as_bytes())?;
            encode_typed_key_value(&mut encoder, value_for(column)?)?;
        }
        Ok(encoder.finish())
    }
}

/// A projection identity could not be encoded without ambiguity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ProjectionScopeCodecError {
    BlankModelRegistration,
    InvalidModelRegistration {
        model: String,
        reason: String,
    },
    ModelRegistrationMismatch {
        declared: String,
        schema: String,
    },
    DuplicateModelRegistration {
        model: String,
    },
    ProjectorMismatch {
        expected: String,
        actual: String,
    },
    StoredScopeMismatch {
        projector: String,
        model: String,
    },
    UnknownModel {
        projector: String,
        model: String,
    },
    DuplicateKeyField {
        model: String,
        field: String,
    },
    ExtraKeyField {
        model: String,
        field: String,
    },
    MissingKeyField {
        model: String,
        field: String,
    },
    ExtraKeyColumn {
        model: String,
        column: String,
    },
    MissingKeyColumn {
        model: String,
        column: String,
    },
    NullPrimaryKey {
        model: String,
        field: String,
    },
    WrongJsonShape {
        model: String,
        field: String,
        expected: &'static str,
        actual: &'static str,
    },
    WrongRowValueShape {
        model: String,
        column: String,
        expected: &'static str,
        actual: &'static str,
    },
    IntegerOutOfRange {
        model: String,
        field: String,
        expected: &'static str,
    },
    NonFiniteFloat {
        model: String,
        field: String,
    },
    InvalidBytes {
        model: String,
        field: String,
    },
    CanonicalEncodingTooLong {
        target: &'static str,
        max: usize,
    },
    Protocol(ProjectionProtocolValidationError),
}

impl fmt::Display for ProjectionScopeCodecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BlankModelRegistration => {
                formatter.write_str("projection model registration must not be blank")
            }
            Self::InvalidModelRegistration { model, reason } => {
                write!(formatter, "invalid projection model `{model}`: {reason}")
            }
            Self::ModelRegistrationMismatch { declared, schema } => write!(
                formatter,
                "projection model registration `{declared}` does not match schema model `{schema}`"
            ),
            Self::DuplicateModelRegistration { model } => {
                write!(
                    formatter,
                    "projection model `{model}` is already registered"
                )
            }
            Self::ProjectorMismatch { expected, actual } => write!(
                formatter,
                "projection scope belongs to projector `{expected}`, not `{actual}`"
            ),
            Self::StoredScopeMismatch { projector, model } => write!(
                formatter,
                "stored projection obligation `{projector}`/`{model}` scope does not match its canonical logical fields"
            ),
            Self::UnknownModel { projector, model } => write!(
                formatter,
                "projector `{projector}` does not register projection model `{model}`"
            ),
            Self::DuplicateKeyField { model, field } => {
                write!(
                    formatter,
                    "projection key for `{model}` repeats field `{field}`"
                )
            }
            Self::ExtraKeyField { model, field } => write!(
                formatter,
                "projection key for `{model}` contains non-key field `{field}`"
            ),
            Self::MissingKeyField { model, field } => {
                write!(
                    formatter,
                    "projection key for `{model}` is missing field `{field}`"
                )
            }
            Self::ExtraKeyColumn { model, column } => write!(
                formatter,
                "projector row key for `{model}` contains non-key column `{column}`"
            ),
            Self::MissingKeyColumn { model, column } => write!(
                formatter,
                "projector row key for `{model}` is missing column `{column}`"
            ),
            Self::NullPrimaryKey { model, field } => {
                write!(
                    formatter,
                    "projection key `{model}.{field}` must not be null"
                )
            }
            Self::WrongJsonShape {
                model,
                field,
                expected,
                actual,
            } => write!(
                formatter,
                "projection key `{model}.{field}` must be {expected}, got {actual}"
            ),
            Self::WrongRowValueShape {
                model,
                column,
                expected,
                actual,
            } => write!(
                formatter,
                "projector row key `{model}.{column}` must be {expected}, got {actual}"
            ),
            Self::IntegerOutOfRange {
                model,
                field,
                expected,
            } => write!(
                formatter,
                "projection key `{model}.{field}` is outside the {expected} range"
            ),
            Self::NonFiniteFloat { model, field } => write!(
                formatter,
                "projection key `{model}.{field}` must be a finite float"
            ),
            Self::InvalidBytes { model, field } => write!(
                formatter,
                "projection key `{model}.{field}` must be canonical standard base64"
            ),
            Self::CanonicalEncodingTooLong { target, max } => {
                write!(formatter, "{target} canonical encoding exceeds {max} bytes")
            }
            Self::Protocol(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ProjectionScopeCodecError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Protocol(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ProjectionProtocolValidationError> for ProjectionScopeCodecError {
    fn from(error: ProjectionProtocolValidationError) -> Self {
        Self::Protocol(error)
    }
}

fn validate_registration_name(model: &str) -> Result<(), ProjectionScopeCodecError> {
    if model.trim().is_empty() {
        return Err(ProjectionScopeCodecError::BlankModelRegistration);
    }
    if model.len() > MAX_PROJECTION_MODEL_NAME_BYTES {
        return Err(ProjectionScopeCodecError::InvalidModelRegistration {
            model: model.to_string(),
            reason: format!(
                "name is {} bytes, exceeding the maximum of {}",
                model.len(),
                MAX_PROJECTION_MODEL_NAME_BYTES
            ),
        });
    }
    if model
        .chars()
        .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(ProjectionScopeCodecError::InvalidModelRegistration {
            model: model.to_string(),
            reason: "name contains whitespace or a control character".into(),
        });
    }
    Ok(())
}

fn validate_model_schema(schema: &TableSchema) -> Result<(), ProjectionScopeCodecError> {
    schema.validate().map_err(
        |error| ProjectionScopeCodecError::InvalidModelRegistration {
            model: schema.model_name.clone(),
            reason: error.to_string(),
        },
    )?;
    if !matches!(schema.kind, TableKind::ReadModel) {
        return Err(ProjectionScopeCodecError::InvalidModelRegistration {
            model: schema.model_name.clone(),
            reason: "projection models must be read models".into(),
        });
    }

    let mut primary_key_columns = BTreeSet::new();
    let mut primary_key_fields = BTreeSet::new();
    for column_name in &schema.primary_key.columns {
        if !primary_key_columns.insert(column_name.as_str()) {
            return Err(ProjectionScopeCodecError::InvalidModelRegistration {
                model: schema.model_name.clone(),
                reason: format!("primary key repeats column `{column_name}`"),
            });
        }
        let column = key_column(schema, column_name).ok_or_else(|| {
            ProjectionScopeCodecError::InvalidModelRegistration {
                model: schema.model_name.clone(),
                reason: format!("primary key references missing column `{column_name}`"),
            }
        })?;
        if !column.primary_key {
            return Err(ProjectionScopeCodecError::InvalidModelRegistration {
                model: schema.model_name.clone(),
                reason: format!(
                    "primary-key list contains column `{column_name}` but the column is not marked primary-key"
                ),
            });
        }
        if column.field_name.trim().is_empty() {
            return Err(ProjectionScopeCodecError::InvalidModelRegistration {
                model: schema.model_name.clone(),
                reason: format!("primary-key column `{column_name}` has a blank field name"),
            });
        }
        if !primary_key_fields.insert(column.field_name.as_str()) {
            return Err(ProjectionScopeCodecError::InvalidModelRegistration {
                model: schema.model_name.clone(),
                reason: format!(
                    "primary key maps multiple columns to field `{}`",
                    column.field_name
                ),
            });
        }
    }
    if let Some(column) = schema.columns.iter().find(|column| {
        column.primary_key && !primary_key_columns.contains(column.column_name.as_str())
    }) {
        return Err(ProjectionScopeCodecError::InvalidModelRegistration {
            model: schema.model_name.clone(),
            reason: format!(
                "column `{}` is marked primary-key but absent from the primary-key list",
                column.column_name
            ),
        });
    }
    Ok(())
}

fn key_column<'a>(schema: &'a TableSchema, column_name: &str) -> Option<&'a TableColumn> {
    schema
        .columns
        .iter()
        .find(|column| column.column_name == column_name)
}

#[derive(Clone, Debug, PartialEq)]
enum TypedKeyValue {
    Text(String),
    Boolean(bool),
    Integer(i64),
    UnsignedInteger(u64),
    Float(f64),
    Bytes(Vec<u8>),
    Json(serde_json::Value),
    Timestamp(String),
}

fn typed_value_from_json(
    schema: &TableSchema,
    column: &TableColumn,
    value: &serde_json::Value,
) -> Result<TypedKeyValue, ProjectionScopeCodecError> {
    if value.is_null() {
        return Err(ProjectionScopeCodecError::NullPrimaryKey {
            model: schema.model_name.clone(),
            field: column.field_name.clone(),
        });
    }

    let wrong_shape = |expected| ProjectionScopeCodecError::WrongJsonShape {
        model: schema.model_name.clone(),
        field: column.field_name.clone(),
        expected,
        actual: json_shape(value),
    };
    match &column.column_type {
        ColumnType::Text => value
            .as_str()
            .map(|value| TypedKeyValue::Text(value.to_string()))
            .ok_or_else(|| wrong_shape("a string")),
        ColumnType::Boolean => value
            .as_bool()
            .map(TypedKeyValue::Boolean)
            .ok_or_else(|| wrong_shape("a boolean")),
        ColumnType::Integer => value.as_i64().map(TypedKeyValue::Integer).ok_or_else(|| {
            ProjectionScopeCodecError::IntegerOutOfRange {
                model: schema.model_name.clone(),
                field: column.field_name.clone(),
                expected: "signed 64-bit integer",
            }
        }),
        ColumnType::UnsignedInteger => value
            .as_u64()
            .map(TypedKeyValue::UnsignedInteger)
            .ok_or_else(|| ProjectionScopeCodecError::IntegerOutOfRange {
                model: schema.model_name.clone(),
                field: column.field_name.clone(),
                expected: "unsigned 64-bit integer",
            }),
        ColumnType::Float => value
            .as_f64()
            .filter(|value| value.is_finite())
            .map(TypedKeyValue::Float)
            .ok_or_else(|| wrong_shape("a finite number")),
        ColumnType::Bytes => {
            let encoded = value
                .as_str()
                .ok_or_else(|| wrong_shape("a base64 string"))?;
            let decoded = BASE64_STANDARD.decode(encoded).map_err(|_| {
                ProjectionScopeCodecError::InvalidBytes {
                    model: schema.model_name.clone(),
                    field: column.field_name.clone(),
                }
            })?;
            if BASE64_STANDARD.encode(&decoded) != encoded {
                return Err(ProjectionScopeCodecError::InvalidBytes {
                    model: schema.model_name.clone(),
                    field: column.field_name.clone(),
                });
            }
            Ok(TypedKeyValue::Bytes(decoded))
        }
        ColumnType::Json => Ok(TypedKeyValue::Json(value.clone())),
        ColumnType::Timestamp => value
            .as_str()
            .map(|value| TypedKeyValue::Timestamp(value.to_string()))
            .ok_or_else(|| wrong_shape("a timestamp string")),
        ColumnType::Unsupported(type_name) => {
            Err(ProjectionScopeCodecError::InvalidModelRegistration {
                model: schema.model_name.clone(),
                reason: format!(
                    "primary-key field `{}` has unsupported type `{type_name}`",
                    column.field_name
                ),
            })
        }
    }
}

fn row_value_from_graphql_json(
    schema: &TableSchema,
    column: &TableColumn,
    value: &serde_json::Value,
) -> Result<RowValue, ProjectionScopeCodecError> {
    if value.is_null() {
        return Err(ProjectionScopeCodecError::NullPrimaryKey {
            model: schema.model_name.clone(),
            field: column.field_name.clone(),
        });
    }

    let integer_out_of_range = |expected| ProjectionScopeCodecError::IntegerOutOfRange {
        model: schema.model_name.clone(),
        field: column.field_name.clone(),
        expected,
    };
    let typed = match &column.column_type {
        ColumnType::Integer => match value {
            serde_json::Value::String(value) => {
                let parsed = value
                    .parse::<i64>()
                    .map_err(|_| integer_out_of_range("signed 64-bit integer"))?;
                if parsed.to_string() != *value {
                    return Err(integer_out_of_range(
                        "canonical signed 64-bit integer string",
                    ));
                }
                TypedKeyValue::Integer(parsed)
            }
            _ => typed_value_from_json(schema, column, value)?,
        },
        ColumnType::UnsignedInteger => match value {
            serde_json::Value::String(value) => {
                let parsed = value
                    .parse::<u64>()
                    .map_err(|_| integer_out_of_range("unsigned 64-bit integer"))?;
                if parsed.to_string() != *value {
                    return Err(integer_out_of_range(
                        "canonical unsigned 64-bit integer string",
                    ));
                }
                TypedKeyValue::UnsignedInteger(parsed)
            }
            _ => typed_value_from_json(schema, column, value)?,
        },
        // SQLite's JSON1 extension exposes BOOLEAN-affinity columns as the
        // lossless integer values 0/1. Accept exactly those private evidence
        // representations in addition to native JSON booleans.
        ColumnType::Boolean => match value.as_i64() {
            Some(0) => TypedKeyValue::Boolean(false),
            Some(1) => TypedKeyValue::Boolean(true),
            _ => typed_value_from_json(schema, column, value)?,
        },
        _ => typed_value_from_json(schema, column, value)?,
    };

    Ok(match typed {
        TypedKeyValue::Text(value) | TypedKeyValue::Timestamp(value) => RowValue::String(value),
        TypedKeyValue::Boolean(value) => RowValue::Bool(value),
        TypedKeyValue::Integer(value) => RowValue::I64(value),
        TypedKeyValue::UnsignedInteger(value) => RowValue::U64(value),
        TypedKeyValue::Float(value) => RowValue::F64(value),
        TypedKeyValue::Bytes(value) => RowValue::Bytes(value),
        TypedKeyValue::Json(value) => RowValue::Json(value),
    })
}

fn typed_value_from_row(
    schema: &TableSchema,
    column: &TableColumn,
    value: &RowValue,
) -> Result<TypedKeyValue, ProjectionScopeCodecError> {
    if matches!(
        value,
        RowValue::Null | RowValue::Json(serde_json::Value::Null)
    ) {
        return Err(ProjectionScopeCodecError::NullPrimaryKey {
            model: schema.model_name.clone(),
            field: column.field_name.clone(),
        });
    }

    let wrong_shape = |expected| ProjectionScopeCodecError::WrongRowValueShape {
        model: schema.model_name.clone(),
        column: column.column_name.clone(),
        expected,
        actual: row_value_shape(value),
    };
    match (&column.column_type, value) {
        (ColumnType::Text, RowValue::String(value)) => Ok(TypedKeyValue::Text(value.clone())),
        (ColumnType::Boolean, RowValue::Bool(value)) => Ok(TypedKeyValue::Boolean(*value)),
        (ColumnType::Integer, RowValue::I64(value)) => Ok(TypedKeyValue::Integer(*value)),
        (ColumnType::UnsignedInteger, RowValue::U64(value)) => {
            Ok(TypedKeyValue::UnsignedInteger(*value))
        }
        (ColumnType::Float, RowValue::F64(value)) if value.is_finite() => {
            Ok(TypedKeyValue::Float(*value))
        }
        (ColumnType::Float, RowValue::F64(_)) => Err(ProjectionScopeCodecError::NonFiniteFloat {
            model: schema.model_name.clone(),
            field: column.field_name.clone(),
        }),
        (ColumnType::Bytes, RowValue::Bytes(value)) => Ok(TypedKeyValue::Bytes(value.clone())),
        (ColumnType::Json, RowValue::Json(value)) => Ok(TypedKeyValue::Json(value.clone())),
        (ColumnType::Timestamp, RowValue::String(value)) => {
            Ok(TypedKeyValue::Timestamp(value.clone()))
        }
        (ColumnType::Unsupported(type_name), _) => {
            Err(ProjectionScopeCodecError::InvalidModelRegistration {
                model: schema.model_name.clone(),
                reason: format!(
                    "primary-key column `{}` has unsupported type `{type_name}`",
                    column.column_name
                ),
            })
        }
        (column_type, _) => Err(wrong_shape(row_value_expectation(column_type))),
    }
}

fn encode_typed_key_value(
    encoder: &mut CanonicalEncoder,
    value: TypedKeyValue,
) -> Result<(), ProjectionScopeCodecError> {
    match value {
        TypedKeyValue::Text(value) => {
            encoder.push_tag(0)?;
            encoder.push_bytes(value.as_bytes())
        }
        TypedKeyValue::Boolean(value) => {
            encoder.push_tag(1)?;
            encoder.push_tag(u8::from(value))
        }
        TypedKeyValue::Integer(value) => {
            encoder.push_tag(2)?;
            encoder.push_raw(&value.to_be_bytes())
        }
        TypedKeyValue::UnsignedInteger(value) => {
            encoder.push_tag(3)?;
            encoder.push_raw(&value.to_be_bytes())
        }
        TypedKeyValue::Float(value) => {
            encoder.push_tag(4)?;
            let canonical = if value == 0.0 { 0.0 } else { value };
            encoder.push_raw(&canonical.to_bits().to_be_bytes())
        }
        TypedKeyValue::Bytes(value) => {
            encoder.push_tag(5)?;
            encoder.push_bytes(&value)
        }
        TypedKeyValue::Json(value) => {
            encoder.push_tag(6)?;
            encode_json(encoder, &value)
        }
        TypedKeyValue::Timestamp(value) => {
            encoder.push_tag(7)?;
            encoder.push_bytes(value.as_bytes())
        }
    }
}

fn encode_json(
    encoder: &mut CanonicalEncoder,
    value: &serde_json::Value,
) -> Result<(), ProjectionScopeCodecError> {
    match value {
        serde_json::Value::Null => encoder.push_tag(0),
        serde_json::Value::Bool(false) => encoder.push_tag(1),
        serde_json::Value::Bool(true) => encoder.push_tag(2),
        serde_json::Value::Number(value) => {
            encoder.push_tag(3)?;
            encoder.push_bytes(value.to_string().as_bytes())
        }
        serde_json::Value::String(value) => {
            encoder.push_tag(4)?;
            encoder.push_bytes(value.as_bytes())
        }
        serde_json::Value::Array(values) => {
            encoder.push_tag(5)?;
            encoder.push_len(values.len())?;
            for value in values {
                encode_json(encoder, value)?;
            }
            Ok(())
        }
        serde_json::Value::Object(values) => {
            encoder.push_tag(6)?;
            encoder.push_len(values.len())?;
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(right.0));
            for (key, value) in entries {
                encoder.push_bytes(key.as_bytes())?;
                encode_json(encoder, value)?;
            }
            Ok(())
        }
    }
}

struct CanonicalEncoder {
    target: &'static str,
    max: usize,
    bytes: Vec<u8>,
}

impl CanonicalEncoder {
    fn new(
        target: &'static str,
        domain: &[u8],
        max: usize,
    ) -> Result<Self, ProjectionScopeCodecError> {
        let mut encoder = Self {
            target,
            max,
            bytes: Vec::with_capacity(domain.len() + 32),
        };
        encoder.push_raw(domain)?;
        Ok(encoder)
    }

    fn push_tag(&mut self, tag: u8) -> Result<(), ProjectionScopeCodecError> {
        self.push_raw(&[tag])
    }

    fn push_len(&mut self, len: usize) -> Result<(), ProjectionScopeCodecError> {
        self.push_raw(&(len as u64).to_be_bytes())
    }

    fn push_bytes(&mut self, value: &[u8]) -> Result<(), ProjectionScopeCodecError> {
        self.push_len(value.len())?;
        self.push_raw(value)
    }

    fn push_raw(&mut self, value: &[u8]) -> Result<(), ProjectionScopeCodecError> {
        if self.bytes.len().saturating_add(value.len()) > self.max {
            return Err(ProjectionScopeCodecError::CanonicalEncodingTooLong {
                target: self.target,
                max: self.max,
            });
        }
        self.bytes.extend_from_slice(value);
        Ok(())
    }

    fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

fn json_shape(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

fn row_value_shape(value: &RowValue) -> &'static str {
    match value {
        RowValue::Null => "null",
        RowValue::Bool(_) => "boolean",
        RowValue::I64(_) => "signed integer",
        RowValue::U64(_) => "unsigned integer",
        RowValue::F64(_) => "float",
        RowValue::String(_) => "string",
        RowValue::Bytes(_) => "bytes",
        RowValue::Json(_) => "json",
    }
}

fn row_value_expectation(column_type: &ColumnType) -> &'static str {
    match column_type {
        ColumnType::Text | ColumnType::Timestamp => "a string RowValue",
        ColumnType::Boolean => "a boolean RowValue",
        ColumnType::Integer => "a signed-integer RowValue",
        ColumnType::UnsignedInteger => "an unsigned-integer RowValue",
        ColumnType::Float => "a finite-float RowValue",
        ColumnType::Bytes => "a bytes RowValue",
        ColumnType::Json => "a JSON RowValue",
        ColumnType::Unsupported(_) => "a supported RowValue",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection_protocol::{
        ResolvedProjectionKey, ResolvedProjectionKeyField, ResolvedProjectionObligation,
    };
    use crate::table::{PrimaryKey, TableColumn};

    fn topology() -> ProjectorTopologyId {
        ProjectorTopologyId::new(3, "project_memberships", [7; 32]).unwrap()
    }

    fn schema(
        model: &str,
        table: &str,
        columns: Vec<TableColumn>,
        primary_key: &[&str],
    ) -> &'static TableSchema {
        Box::leak(Box::new(TableSchema {
            model_name: model.into(),
            table_name: table.into(),
            columns,
            primary_key: PrimaryKey::new(primary_key.iter().copied()),
            version_column: Some("_sourced_version".into()),
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships: Vec::new(),
            kind: TableKind::ReadModel,
        }))
    }

    fn key_column(field: &str, column: &str, column_type: ColumnType) -> TableColumn {
        TableColumn {
            primary_key: true,
            ..TableColumn::new(field, column, column_type)
        }
    }

    fn obligation(
        codec: &ProjectionScopeCodec,
        projector: &str,
        model: &str,
        partition: Option<serde_json::Value>,
        fields: impl IntoIterator<Item = (&'static str, serde_json::Value)>,
    ) -> ResolvedProjectionObligation {
        let key = ResolvedProjectionKey {
            fields: fields
                .into_iter()
                .map(|(field, value)| ResolvedProjectionKeyField {
                    field: field.into(),
                    value,
                })
                .collect(),
        };
        let scope = codec
            .encode_resolved_obligation_scope(projector, model, &key, partition.as_ref())
            .unwrap_or_else(|_| {
                ProjectionRecordScope::new(
                    codec.topology().clone(),
                    codec.encode_partition(partition.as_ref()).unwrap(),
                    model,
                    b"invalid-test-key".to_vec(),
                )
                .unwrap()
            });
        ResolvedProjectionObligation {
            projector: projector.into(),
            model: model.into(),
            partition,
            key,
            scope,
        }
    }

    #[test]
    fn obligation_and_row_keys_share_one_schema_ordered_composite_encoding() {
        let membership = schema(
            "Membership",
            "memberships",
            vec![
                key_column("tenantId", "tenant_id", ColumnType::Text),
                key_column("sequence", "member_sequence", ColumnType::UnsignedInteger),
                key_column("attributes", "attributes_json", ColumnType::Json),
            ],
            &["tenant_id", "member_sequence", "attributes_json"],
        );
        let codec =
            ProjectionScopeCodec::with_models(topology(), [("Membership", membership)]).unwrap();
        let command_partition =
            serde_json::from_str(r#"{"region":"west","tenant":{"id":"t-1","tier":2}}"#).unwrap();
        let projector_partition =
            serde_json::from_str(r#"{"tenant":{"tier":2,"id":"t-1"},"region":"west"}"#).unwrap();
        let command = obligation(
            &codec,
            "project_memberships",
            "Membership",
            Some(command_partition),
            [
                (
                    "attributes",
                    serde_json::json!({"z": [2, 1], "a": {"b": true}}),
                ),
                ("sequence", serde_json::json!(42_u64)),
                ("tenantId", serde_json::json!("t-1")),
            ],
        );
        let row = RowKey::new([
            ("tenant_id", RowValue::String("t-1".into())),
            ("member_sequence", RowValue::U64(42)),
            (
                "attributes_json",
                RowValue::Json(serde_json::json!({"a": {"b": true}, "z": [2, 1]})),
            ),
        ]);

        let obligation_scope = codec.encode_obligation_scope(&command).unwrap();
        let row_scope = codec
            .encode_row_scope(
                "project_memberships",
                "Membership",
                Some(&projector_partition),
                &row,
            )
            .unwrap();

        assert_eq!(obligation_scope, row_scope);
        assert_eq!(
            obligation_scope.canonical_key_bytes(),
            row_scope.canonical_key_bytes()
        );
        assert_eq!(obligation_scope.key_digest(), row_scope.key_digest());
    }

    #[test]
    fn recursive_json_is_object_order_invariant_and_absent_is_not_null() {
        let codec = ProjectionScopeCodec::new(topology());
        let left =
            serde_json::from_str(r#"{"z":{"b":2,"a":1},"a":[{"y":true,"x":null}]}"#).unwrap();
        let right =
            serde_json::from_str(r#"{"a":[{"x":null,"y":true}],"z":{"a":1,"b":2}}"#).unwrap();

        assert_eq!(
            codec.encode_partition(Some(&left)).unwrap(),
            codec.encode_partition(Some(&right)).unwrap()
        );

        let absent = codec.encode_partition(None).unwrap();
        let explicit_null = codec
            .encode_partition(Some(&serde_json::Value::Null))
            .unwrap();
        assert_ne!(absent.canonical_bytes(), explicit_null.canonical_bytes());
        assert_ne!(absent.digest(), explicit_null.digest());
    }

    #[test]
    fn integer_schema_controls_signedness_and_range() {
        let signed = schema(
            "SignedRecord",
            "signed_records",
            vec![key_column("id", "id", ColumnType::Integer)],
            &["id"],
        );
        let unsigned = schema(
            "UnsignedRecord",
            "unsigned_records",
            vec![key_column("id", "id", ColumnType::UnsignedInteger)],
            &["id"],
        );
        let codec = ProjectionScopeCodec::with_models(
            topology(),
            [("SignedRecord", signed), ("UnsignedRecord", unsigned)],
        )
        .unwrap();

        let signed_command = obligation(
            &codec,
            "project_memberships",
            "SignedRecord",
            None,
            [("id", serde_json::json!(-1))],
        );
        let signed_row = RowKey::new([("id", RowValue::I64(-1))]);
        assert_eq!(
            codec.encode_obligation_scope(&signed_command).unwrap(),
            codec
                .encode_row_scope("project_memberships", "SignedRecord", None, &signed_row)
                .unwrap()
        );

        let unsigned_command = obligation(
            &codec,
            "project_memberships",
            "UnsignedRecord",
            None,
            [("id", serde_json::json!(u64::MAX))],
        );
        let unsigned_row = RowKey::new([("id", RowValue::U64(u64::MAX))]);
        assert_eq!(
            codec.encode_obligation_scope(&unsigned_command).unwrap(),
            codec
                .encode_row_scope("project_memberships", "UnsignedRecord", None, &unsigned_row)
                .unwrap()
        );

        let negative_unsigned = obligation(
            &codec,
            "project_memberships",
            "UnsignedRecord",
            None,
            [("id", serde_json::json!(-1))],
        );
        assert!(matches!(
            codec.encode_obligation_scope(&negative_unsigned),
            Err(ProjectionScopeCodecError::IntegerOutOfRange {
                expected: "unsigned 64-bit integer",
                ..
            })
        ));

        let too_large_signed = obligation(
            &codec,
            "project_memberships",
            "SignedRecord",
            None,
            [("id", serde_json::json!(u64::MAX))],
        );
        assert!(matches!(
            codec.encode_obligation_scope(&too_large_signed),
            Err(ProjectionScopeCodecError::IntegerOutOfRange {
                expected: "signed 64-bit integer",
                ..
            })
        ));
        assert!(matches!(
            codec.encode_row_scope(
                "project_memberships",
                "SignedRecord",
                None,
                &RowKey::new([("id", RowValue::U64(1))]),
            ),
            Err(ProjectionScopeCodecError::WrongRowValueShape { .. })
        ));
        assert!(matches!(
            codec.encode_row_scope(
                "project_memberships",
                "UnsignedRecord",
                None,
                &RowKey::new([("id", RowValue::I64(1))]),
            ),
            Err(ProjectionScopeCodecError::WrongRowValueShape { .. })
        ));
    }

    #[test]
    fn registration_and_topology_mismatches_fail_closed() {
        let record = schema(
            "Record",
            "records",
            vec![key_column("id", "record_id", ColumnType::Text)],
            &["record_id"],
        );
        let mismatched = schema(
            "Actual",
            "actual",
            vec![key_column("id", "id", ColumnType::Text)],
            &["id"],
        );
        let malformed = schema(
            "Malformed",
            "malformed",
            vec![TableColumn::new("id", "id", ColumnType::Text)],
            &["id"],
        );
        let mut codec = ProjectionScopeCodec::new(topology());

        assert_eq!(
            codec.register_model("", record).unwrap_err(),
            ProjectionScopeCodecError::BlankModelRegistration
        );
        assert!(matches!(
            codec.register_model("Declared", mismatched),
            Err(ProjectionScopeCodecError::ModelRegistrationMismatch { .. })
        ));
        assert!(matches!(
            codec.register_model("Malformed", malformed),
            Err(ProjectionScopeCodecError::InvalidModelRegistration { .. })
        ));
        codec.register_model("Record", record).unwrap();
        assert!(matches!(
            codec.register_model("Record", record),
            Err(ProjectionScopeCodecError::DuplicateModelRegistration { .. })
        ));

        let wrong_projector = obligation(
            &codec,
            "another_projector",
            "Record",
            None,
            [("id", serde_json::json!("r-1"))],
        );
        assert!(matches!(
            codec.encode_obligation_scope(&wrong_projector),
            Err(ProjectionScopeCodecError::ProjectorMismatch { .. })
        ));
        let unknown_model = obligation(
            &codec,
            "project_memberships",
            "Unknown",
            None,
            [("id", serde_json::json!("r-1"))],
        );
        assert!(matches!(
            codec.encode_obligation_scope(&unknown_model),
            Err(ProjectionScopeCodecError::UnknownModel { .. })
        ));
        assert!(matches!(
            codec.encode_row_scope(
                "another_projector",
                "Record",
                None,
                &RowKey::new([("record_id", RowValue::String("r-1".into()))]),
            ),
            Err(ProjectionScopeCodecError::ProjectorMismatch { .. })
        ));
    }

    #[test]
    fn malformed_obligation_and_row_keys_fail_closed() {
        let record = schema(
            "Record",
            "records",
            vec![
                key_column("id", "record_id", ColumnType::Text),
                key_column("active", "is_active", ColumnType::Boolean),
            ],
            &["record_id", "is_active"],
        );
        let float_record = schema(
            "FloatRecord",
            "float_records",
            vec![key_column("id", "id", ColumnType::Float)],
            &["id"],
        );
        let bytes_record = schema(
            "BytesRecord",
            "bytes_records",
            vec![key_column("id", "id", ColumnType::Bytes)],
            &["id"],
        );
        let codec = ProjectionScopeCodec::with_models(
            topology(),
            [
                ("Record", record),
                ("FloatRecord", float_record),
                ("BytesRecord", bytes_record),
            ],
        )
        .unwrap();

        let missing = obligation(
            &codec,
            "project_memberships",
            "Record",
            None,
            [("id", serde_json::json!("r-1"))],
        );
        assert!(matches!(
            codec.encode_obligation_scope(&missing),
            Err(ProjectionScopeCodecError::MissingKeyField { field, .. })
                if field == "active"
        ));

        let extra = obligation(
            &codec,
            "project_memberships",
            "Record",
            None,
            [
                ("id", serde_json::json!("r-1")),
                ("active", serde_json::json!(true)),
                ("other", serde_json::json!(1)),
            ],
        );
        assert!(matches!(
            codec.encode_obligation_scope(&extra),
            Err(ProjectionScopeCodecError::ExtraKeyField { field, .. })
                if field == "other"
        ));

        let duplicate = obligation(
            &codec,
            "project_memberships",
            "Record",
            None,
            [
                ("id", serde_json::json!("r-1")),
                ("id", serde_json::json!("r-2")),
                ("active", serde_json::json!(true)),
            ],
        );
        assert!(matches!(
            codec.encode_obligation_scope(&duplicate),
            Err(ProjectionScopeCodecError::DuplicateKeyField { field, .. })
                if field == "id"
        ));

        let null = obligation(
            &codec,
            "project_memberships",
            "Record",
            None,
            [
                ("id", serde_json::Value::Null),
                ("active", serde_json::json!(true)),
            ],
        );
        assert!(matches!(
            codec.encode_obligation_scope(&null),
            Err(ProjectionScopeCodecError::NullPrimaryKey { field, .. })
                if field == "id"
        ));

        let wrong_json_shape = obligation(
            &codec,
            "project_memberships",
            "Record",
            None,
            [
                ("id", serde_json::json!("r-1")),
                ("active", serde_json::json!("true")),
            ],
        );
        assert!(matches!(
            codec.encode_obligation_scope(&wrong_json_shape),
            Err(ProjectionScopeCodecError::WrongJsonShape { field, .. })
                if field == "active"
        ));

        assert!(matches!(
            codec.encode_row_scope(
                "project_memberships",
                "Record",
                None,
                &RowKey::new([
                    ("record_id", RowValue::String("r-1".into())),
                    ("is_active", RowValue::String("true".into())),
                ]),
            ),
            Err(ProjectionScopeCodecError::WrongRowValueShape { column, .. })
                if column == "is_active"
        ));
        assert!(matches!(
            codec.encode_row_scope(
                "project_memberships",
                "Record",
                None,
                &RowKey::new([
                    ("record_id", RowValue::String("r-1".into())),
                    ("is_active", RowValue::Bool(true)),
                    ("other", RowValue::I64(1)),
                ]),
            ),
            Err(ProjectionScopeCodecError::ExtraKeyColumn { column, .. })
                if column == "other"
        ));
        assert!(matches!(
            codec.encode_row_scope(
                "project_memberships",
                "Record",
                None,
                &RowKey::new([("record_id", RowValue::String("r-1".into()))]),
            ),
            Err(ProjectionScopeCodecError::MissingKeyColumn { column, .. })
                if column == "is_active"
        ));
        assert!(matches!(
            codec.encode_row_scope(
                "project_memberships",
                "FloatRecord",
                None,
                &RowKey::new([("id", RowValue::F64(f64::NAN))]),
            ),
            Err(ProjectionScopeCodecError::NonFiniteFloat { .. })
        ));

        let noncanonical_base64 = obligation(
            &codec,
            "project_memberships",
            "BytesRecord",
            None,
            [("id", serde_json::json!("AB=="))],
        );
        assert!(matches!(
            codec.encode_obligation_scope(&noncanonical_base64),
            Err(ProjectionScopeCodecError::InvalidBytes { .. })
        ));
    }

    #[test]
    fn codec_owns_registered_schema_independently_of_the_caller() {
        let mut original = TableSchema {
            model_name: "OwnedRecord".into(),
            table_name: "owned_records".into(),
            columns: vec![key_column("id", "record_id", ColumnType::Text)],
            primary_key: PrimaryKey::new(["record_id"]),
            version_column: None,
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships: Vec::new(),
            kind: TableKind::ReadModel,
        };
        let codec =
            ProjectionScopeCodec::with_models(topology(), [("OwnedRecord", &original)]).unwrap();

        original.model_name = "MutatedAfterRegistration".into();
        original.table_name = "mutated_after_registration".into();
        original.primary_key = PrimaryKey::new(["not_the_registered_key"]);
        drop(original);

        let registered = codec.registered_schema("OwnedRecord").unwrap();
        assert_eq!(registered.model_name, "OwnedRecord");
        assert_eq!(registered.table_name, "owned_records");
        assert_eq!(registered.primary_key.columns, ["record_id"]);
        assert_eq!(
            codec
                .registered_schema_owned("OwnedRecord")
                .unwrap()
                .table_name,
            "owned_records"
        );
    }

    #[test]
    fn graphql_json_columns_decode_lossless_composite_row_keys() {
        let composite = TableSchema {
            model_name: "CompositeRecord".into(),
            table_name: "composite_records".into(),
            columns: vec![
                key_column("signed", "signed_id", ColumnType::Integer),
                key_column("unsigned", "unsigned_id", ColumnType::UnsignedInteger),
                key_column("active", "is_active", ColumnType::Boolean),
                key_column("digest", "digest_bytes", ColumnType::Bytes),
                key_column("attributes", "attributes_json", ColumnType::Json),
            ],
            primary_key: PrimaryKey::new([
                "signed_id",
                "unsigned_id",
                "is_active",
                "digest_bytes",
                "attributes_json",
            ]),
            version_column: None,
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships: Vec::new(),
            kind: TableKind::ReadModel,
        };
        let codec =
            ProjectionScopeCodec::with_models(topology(), [("CompositeRecord", &composite)])
                .unwrap();
        let values = BTreeMap::from([
            (
                "signed_id".into(),
                serde_json::Value::String(i64::MIN.to_string()),
            ),
            (
                "unsigned_id".into(),
                serde_json::Value::String(u64::MAX.to_string()),
            ),
            ("is_active".into(), serde_json::json!(1)),
            ("digest_bytes".into(), serde_json::json!("AP8=")),
            (
                "attributes_json".into(),
                serde_json::json!({"z": [2, 1], "a": true}),
            ),
        ]);

        let decoded = codec
            .row_key_from_json_columns("CompositeRecord", &values)
            .unwrap();
        let expected = RowKey::new([
            ("signed_id", RowValue::I64(i64::MIN)),
            ("unsigned_id", RowValue::U64(u64::MAX)),
            ("is_active", RowValue::Bool(true)),
            ("digest_bytes", RowValue::Bytes(vec![0, 255])),
            (
                "attributes_json",
                RowValue::Json(serde_json::json!({"a": true, "z": [2, 1]})),
            ),
        ]);
        assert_eq!(decoded, expected);
        assert_eq!(
            codec
                .encode_unpartitioned_row_key("CompositeRecord", &decoded)
                .unwrap(),
            codec
                .encode_unpartitioned_row_key("CompositeRecord", &expected)
                .unwrap()
        );

        let mut missing = values.clone();
        missing.remove("digest_bytes");
        assert!(matches!(
            codec.row_key_from_json_columns("CompositeRecord", &missing),
            Err(ProjectionScopeCodecError::MissingKeyColumn { column, .. })
                if column == "digest_bytes"
        ));

        let mut extra = values.clone();
        extra.insert("other".into(), serde_json::json!(1));
        assert!(matches!(
            codec.row_key_from_json_columns("CompositeRecord", &extra),
            Err(ProjectionScopeCodecError::ExtraKeyColumn { column, .. })
                if column == "other"
        ));

        let mut noncanonical_integer = values.clone();
        noncanonical_integer.insert("signed_id".into(), serde_json::json!("01"));
        assert!(matches!(
            codec.row_key_from_json_columns("CompositeRecord", &noncanonical_integer),
            Err(ProjectionScopeCodecError::IntegerOutOfRange { .. })
        ));

        let mut noncanonical_bytes = values;
        noncanonical_bytes.insert("digest_bytes".into(), serde_json::json!("AB=="));
        assert!(matches!(
            codec.row_key_from_json_columns("CompositeRecord", &noncanonical_bytes),
            Err(ProjectionScopeCodecError::InvalidBytes { .. })
        ));
    }
}
