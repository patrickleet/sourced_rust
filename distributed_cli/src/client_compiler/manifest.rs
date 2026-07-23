use std::collections::{BTreeMap, BTreeSet};

use async_graphql_parser::types::{DocumentOperations, OperationType, Selection};
use async_graphql_value::Value;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};

use super::{ClientCompileError, ClientSurfaceSelector};

const MANIFEST_VERSION: u64 = 4;
const PROTOCOL_VERSION: u64 = 2;
const PROTOCOL_FINGERPRINT: &str =
    "sha256:50a3690689ff5aa7cefc88bb7b5d6f1e1a64615e7644d306403287c09b1e59dc";

#[derive(Clone, Debug)]
pub(crate) struct ClientManifest {
    pub(crate) service_id: String,
    pub(crate) schema_fingerprint: String,
    pub(crate) protocol_fingerprint: String,
    pub(crate) capabilities: ManifestCapabilities,
    pub(crate) scalar_codecs: BTreeMap<String, String>,
    pub(crate) models: BTreeMap<String, ManifestModel>,
    pub(crate) roots: BTreeMap<(RootOperation, String), ManifestRoot>,
    pub(crate) commands: Vec<JsonValue>,
    pub(crate) protocol_operations: JsonValue,
    pub(crate) projectors: Vec<JsonValue>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ManifestSurface {
    Role { name: String },
    Application { name: String, roles: Vec<String> },
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ManifestCapabilities {
    pub(crate) live_queries: bool,
    pub(crate) causal_receipts: bool,
}

#[derive(Clone, Debug, Deserialize)]
struct ManifestScalarCodec {
    scalar: String,
    codec: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ManifestModel {
    pub(crate) id: String,
    pub(crate) typename: String,
    pub(crate) normalization: ManifestNormalization,
    pub(crate) fields: Vec<ManifestField>,
    pub(crate) relationships: Vec<ManifestRelationship>,
}

impl ManifestModel {
    pub(crate) fn field(&self, name: &str) -> Option<&ManifestField> {
        self.fields.iter().find(|field| field.name == name)
    }

    pub(crate) fn relationship(&self, name: &str) -> Option<&ManifestRelationship> {
        self.relationships
            .iter()
            .find(|relationship| relationship.name == name)
    }

    pub(crate) fn identity(&self) -> Option<&[ManifestKeyField]> {
        match &self.normalization {
            ManifestNormalization::Normalized { fields, .. } => Some(fields),
            ManifestNormalization::Embedded => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ManifestNormalization {
    Normalized {
        fields: Vec<ManifestKeyField>,
        encoding: String,
    },
    Embedded,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ManifestKeyField {
    pub(crate) name: String,
    pub(crate) codec: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ManifestField {
    pub(crate) name: String,
    pub(crate) scalar: String,
    pub(crate) codec: String,
    pub(crate) nullable: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ManifestRelationship {
    pub(crate) name: String,
    pub(crate) nullable: bool,
    pub(crate) list: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RootOperation {
    Query,
    Subscription,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RootKind {
    List,
    ByPk,
    Aggregate,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ManifestRoot {
    pub(crate) id: String,
    pub(crate) operation: RootOperation,
    pub(crate) name: String,
    pub(crate) kind: RootKind,
    pub(crate) model: String,
    pub(crate) arguments: Vec<ManifestArgument>,
    pub(crate) dependencies: Vec<String>,
    pub(crate) live: bool,
    pub(crate) pagination: Option<ManifestPagination>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ManifestArgument {
    pub(crate) name: String,
    pub(crate) kind: ManifestArgumentKind,
    pub(crate) type_name: String,
    pub(crate) nullable: bool,
    pub(crate) list: bool,
    pub(crate) codec: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ManifestArgumentKind {
    Filter,
    Order,
    Limit,
    Offset,
    PrimaryKey,
}

impl ManifestArgument {
    pub(crate) fn graphql_type(&self) -> String {
        let base = if self.list {
            format!("[{}!]", self.type_name)
        } else {
            self.type_name.clone()
        };
        if self.nullable {
            base
        } else {
            format!("{base}!")
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub(crate) struct ManifestPagination {
    pub(crate) kind: String,
    pub(crate) default_limit: u64,
    pub(crate) max_limit: u64,
    pub(crate) coverage: String,
}

#[derive(Deserialize)]
struct ManifestWire {
    manifest_version: u64,
    protocol_version: u64,
    service_id: String,
    surface: ManifestSurface,
    schema_fingerprint: String,
    protocol_fingerprint: String,
    capabilities: ManifestCapabilities,
    scalar_codecs: Vec<ManifestScalarCodec>,
    models: Vec<ManifestModel>,
    roots: Vec<ManifestRoot>,
    commands: Vec<JsonValue>,
    protocol_operations: JsonValue,
    projectors: Vec<JsonValue>,
}

impl ClientManifest {
    pub(crate) fn parse(
        value: JsonValue,
        selector: &ClientSurfaceSelector,
    ) -> Result<Self, ClientCompileError> {
        let wire: ManifestWire = serde_json::from_value(value).map_err(|error| {
            ClientCompileError::manifest(
                "client.manifest.invalid",
                format!("invalid Distributed client manifest: {error}"),
            )
        })?;
        if wire.manifest_version != MANIFEST_VERSION {
            return Err(ClientCompileError::manifest(
                "client.manifest.version",
                format!(
                    "client compiler requires manifest_version {MANIFEST_VERSION}, received {}",
                    wire.manifest_version
                ),
            ));
        }
        if wire.protocol_version != PROTOCOL_VERSION {
            return Err(ClientCompileError::manifest(
                "client.manifest.protocol_version",
                format!(
                    "client compiler requires protocol_version {PROTOCOL_VERSION}, received {}",
                    wire.protocol_version
                ),
            ));
        }
        validate_nonempty(&wire.service_id, "manifest.service_id")?;
        validate_hash(&wire.schema_fingerprint, "manifest.schema_fingerprint")?;
        validate_hash(&wire.protocol_fingerprint, "manifest.protocol_fingerprint")?;
        if wire.protocol_fingerprint != PROTOCOL_FINGERPRINT {
            return Err(ClientCompileError::manifest(
                "client.manifest.protocol_fingerprint",
                format!(
                    "client compiler protocol contract is `{PROTOCOL_FINGERPRINT}`, received `{}`; regenerate the manifest and use a matching dctl version",
                    wire.protocol_fingerprint
                ),
            ));
        }
        validate_surface(&wire.surface, selector)?;

        let scalar_codecs = validate_scalar_codecs(wire.scalar_codecs)?;
        let mut models = BTreeMap::new();
        let mut typenames = BTreeSet::new();
        for model in wire.models {
            validate_model(&model, &scalar_codecs)?;
            if !typenames.insert(model.typename.clone()) {
                return Err(ClientCompileError::manifest(
                    "client.manifest.duplicate_typename",
                    format!("duplicate manifest model typename `{}`", model.typename),
                ));
            }
            let id = model.id.clone();
            if models.insert(id.clone(), model).is_some() {
                return Err(ClientCompileError::manifest(
                    "client.manifest.duplicate_model",
                    format!("duplicate manifest model id `{id}`"),
                ));
            }
        }

        let mut roots = BTreeMap::new();
        let mut root_ids = BTreeSet::new();
        for mut root in wire.roots {
            validate_nonempty(&root.id, "manifest root id")?;
            validate_nonempty(&root.name, "manifest root name")?;
            validate_nonempty(&root.model, "manifest root model")?;
            if !root_ids.insert(root.id.clone()) {
                return Err(ClientCompileError::manifest(
                    "client.manifest.duplicate_root_id",
                    format!("duplicate manifest root id `{}`", root.id),
                ));
            }
            if !models.contains_key(&root.model) {
                return Err(ClientCompileError::manifest(
                    "client.manifest.root_model",
                    format!(
                        "manifest root `{}` references missing model `{}`",
                        root.name, root.model
                    ),
                ));
            }
            root.dependencies.sort();
            root.dependencies.dedup();
            validate_unique_arguments(&root, &scalar_codecs)?;
            let key = (root.operation, root.name.clone());
            if roots.insert(key, root).is_some() {
                return Err(ClientCompileError::manifest(
                    "client.manifest.duplicate_root",
                    "duplicate manifest operation root",
                ));
            }
        }

        validate_commands(&wire.commands, wire.capabilities.causal_receipts)?;
        validate_protocol_operations(&wire.protocol_operations, !wire.commands.is_empty())?;

        Ok(Self {
            service_id: wire.service_id,
            schema_fingerprint: wire.schema_fingerprint,
            protocol_fingerprint: wire.protocol_fingerprint,
            capabilities: wire.capabilities,
            scalar_codecs,
            models,
            roots,
            commands: canonical_entries(wire.commands, "name"),
            protocol_operations: canonical_json_value(wire.protocol_operations),
            projectors: canonical_entries(wire.projectors, "name"),
        })
    }

    pub(crate) fn root(&self, operation: RootOperation, name: &str) -> Option<&ManifestRoot> {
        self.roots.get(&(operation, name.to_string()))
    }
}

fn validate_surface(
    actual: &ManifestSurface,
    expected: &ClientSurfaceSelector,
) -> Result<(), ClientCompileError> {
    let matches = match (actual, expected) {
        (
            ManifestSurface::Role { name: actual },
            ClientSurfaceSelector::Role { name: expected },
        ) => !expected.trim().is_empty() && actual == expected,
        (
            ManifestSurface::Application {
                name: actual,
                roles,
            },
            ClientSurfaceSelector::Application { name: expected },
        ) => {
            !expected.trim().is_empty()
                && actual == expected
                && !roles.is_empty()
                && roles.iter().all(|role| !role.trim().is_empty())
        }
        _ => false,
    };
    if matches {
        return Ok(());
    }
    let actual_label = match actual {
        ManifestSurface::Role { name } => format!("role `{name}`"),
        ManifestSurface::Application { name, .. } => format!("application `{name}`"),
    };
    let expected_label = match expected {
        ClientSurfaceSelector::Role { name } => format!("role `{name}`"),
        ClientSurfaceSelector::Application { name } => format!("application `{name}`"),
    };
    Err(ClientCompileError::manifest(
        "client.manifest.surface_mismatch",
        format!(
            "selected manifest surface is {actual_label}; compiler was explicitly requested for {expected_label}"
        ),
    ))
}

fn validate_scalar_codecs(
    codecs: Vec<ManifestScalarCodec>,
) -> Result<BTreeMap<String, String>, ClientCompileError> {
    if codecs.is_empty() {
        return Err(ClientCompileError::manifest(
            "client.manifest.scalar_codecs",
            "manifest.scalar_codecs must declare the complete authorized scalar inventory",
        ));
    }
    let supported = [
        ("BigInt", "json_number_precision_limited"),
        ("Boolean", "boolean"),
        ("Bytea", "base64"),
        ("Float", "float64"),
        ("ID", "string"),
        ("Int", "int32"),
        ("JSON", "json"),
        ("String", "string"),
        ("Timestamptz", "string_unvalidated_timestamp"),
    ]
    .into_iter()
    .collect::<BTreeMap<_, _>>();
    let mut result = BTreeMap::new();
    for entry in codecs {
        validate_nonempty(&entry.scalar, "manifest scalar")?;
        validate_nonempty(&entry.codec, "manifest scalar codec")?;
        let expected = supported.get(entry.scalar.as_str()).ok_or_else(|| {
            ClientCompileError::manifest(
                "client.manifest.scalar_unsupported",
                format!(
                    "scalar `{}` has no fail-closed TypeScript codec in this compiler",
                    entry.scalar
                ),
            )
        })?;
        if entry.codec != *expected {
            return Err(ClientCompileError::manifest(
                "client.manifest.codec_unsupported",
                format!(
                    "scalar `{}` declares codec `{}`; compiler requires `{expected}`",
                    entry.scalar, entry.codec
                ),
            ));
        }
        if result
            .insert(entry.scalar.clone(), entry.codec.clone())
            .is_some()
        {
            return Err(ClientCompileError::manifest(
                "client.manifest.duplicate_scalar",
                format!("manifest repeats scalar codec `{}`", entry.scalar),
            ));
        }
    }
    Ok(result)
}

fn validate_model(
    model: &ManifestModel,
    scalar_codecs: &BTreeMap<String, String>,
) -> Result<(), ClientCompileError> {
    validate_nonempty(&model.id, "manifest model id")?;
    validate_nonempty(&model.typename, "manifest model typename")?;
    let mut names = BTreeSet::new();
    for field in &model.fields {
        validate_nonempty(&field.name, "manifest model field")?;
        validate_nonempty(&field.scalar, "manifest field scalar")?;
        validate_nonempty(&field.codec, "manifest field codec")?;
        match scalar_codecs.get(&field.scalar) {
            Some(codec) if codec == &field.codec => {}
            Some(codec) => {
                return Err(ClientCompileError::manifest(
                    "client.manifest.field_codec",
                    format!(
                        "model `{}` field `{}` codec `{}` does not match scalar `{}` inventory codec `{codec}`",
                        model.id, field.name, field.codec, field.scalar
                    ),
                ));
            }
            None => {
                return Err(ClientCompileError::manifest(
                    "client.manifest.field_scalar",
                    format!(
                        "model `{}` field `{}` uses scalar `{}` absent from manifest.scalar_codecs",
                        model.id, field.name, field.scalar
                    ),
                ));
            }
        }
        if !names.insert(field.name.as_str()) {
            return Err(ClientCompileError::manifest(
                "client.manifest.duplicate_field",
                format!("model `{}` repeats field `{}`", model.id, field.name),
            ));
        }
    }
    for relationship in &model.relationships {
        validate_nonempty(&relationship.name, "manifest relationship")?;
        if !names.insert(relationship.name.as_str()) {
            return Err(ClientCompileError::manifest(
                "client.manifest.duplicate_member",
                format!(
                    "model `{}` repeats field/relationship `{}`",
                    model.id, relationship.name
                ),
            ));
        }
        let _ = (relationship.nullable, relationship.list);
    }
    match &model.normalization {
        ManifestNormalization::Embedded => {}
        ManifestNormalization::Normalized { fields, encoding } => {
            if encoding != "canonical_json_tuple_v1" {
                return Err(ClientCompileError::manifest(
                    "client.manifest.identity_encoding",
                    format!(
                        "model `{}` uses unsupported identity encoding `{encoding}`",
                        model.id
                    ),
                ));
            }
            if fields.is_empty() {
                return Err(ClientCompileError::manifest(
                    "client.manifest.empty_identity",
                    format!("normalized model `{}` has no identity fields", model.id),
                ));
            }
            let mut identities = BTreeSet::new();
            for identity in fields {
                if !identities.insert(identity.name.as_str()) {
                    return Err(ClientCompileError::manifest(
                        "client.manifest.duplicate_identity",
                        format!(
                            "model `{}` repeats identity field `{}`",
                            model.id, identity.name
                        ),
                    ));
                }
                let Some(field) = model.field(&identity.name) else {
                    return Err(ClientCompileError::manifest(
                        "client.manifest.identity_field",
                        format!(
                            "model `{}` identity field `{}` is absent from its authorized fields",
                            model.id, identity.name
                        ),
                    ));
                };
                if field.nullable || field.codec != identity.codec {
                    return Err(ClientCompileError::manifest(
                        "client.manifest.identity_codec",
                        format!(
                            "model `{}` identity field `{}` must be non-null and match codec `{}`",
                            model.id, identity.name, identity.codec
                        ),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn validate_unique_arguments(
    root: &ManifestRoot,
    scalar_codecs: &BTreeMap<String, String>,
) -> Result<(), ClientCompileError> {
    let mut names = BTreeSet::new();
    for argument in &root.arguments {
        validate_nonempty(&argument.name, "manifest root argument")?;
        validate_nonempty(&argument.type_name, "manifest root argument type")?;
        if !names.insert(argument.name.as_str()) {
            return Err(ClientCompileError::manifest(
                "client.manifest.duplicate_argument",
                format!(
                    "manifest root `{}` repeats argument `{}`",
                    root.name, argument.name
                ),
            ));
        }
        if matches!(
            argument.kind,
            ManifestArgumentKind::Limit | ManifestArgumentKind::Offset
        ) && (argument.list || argument.type_name != "Int")
        {
            return Err(ClientCompileError::manifest(
                "client.manifest.pagination_argument",
                format!(
                    "manifest root `{}` pagination argument `{}` must use scalar Int",
                    root.name, argument.name
                ),
            ));
        }
        match (scalar_codecs.get(&argument.type_name), &argument.codec) {
            (Some(expected), Some(actual)) if actual == expected => {}
            (Some(expected), Some(actual)) => {
                return Err(ClientCompileError::manifest(
                    "client.manifest.argument_codec",
                    format!(
                        "manifest root `{}` argument `{}` codec `{actual}` does not match scalar `{}` inventory codec `{expected}`",
                        root.name, argument.name, argument.type_name
                    ),
                ));
            }
            (Some(_), None) => {
                return Err(ClientCompileError::manifest(
                    "client.manifest.argument_codec",
                    format!(
                        "manifest root `{}` scalar argument `{}` is missing its codec",
                        root.name, argument.name
                    ),
                ));
            }
            (None, Some(actual)) => {
                return Err(ClientCompileError::manifest(
                    "client.manifest.argument_codec",
                    format!(
                        "manifest root `{}` argument `{}` declares codec `{actual}` for non-scalar type `{}`",
                        root.name, argument.name, argument.type_name
                    ),
                ));
            }
            (None, None) => {}
        }
    }
    Ok(())
}

fn validate_commands(
    commands: &[JsonValue],
    causal_receipts: bool,
) -> Result<(), ClientCompileError> {
    if !commands.is_empty() && !causal_receipts {
        return Err(ClientCompileError::manifest(
            "client.manifest.command_capability",
            "manifest commands require capabilities.causal_receipts",
        ));
    }
    let mut names = BTreeSet::new();
    let mut fields = BTreeSet::new();
    for (index, command) in commands.iter().enumerate() {
        let object = command.as_object().ok_or_else(|| {
            ClientCompileError::manifest(
                "client.manifest.command",
                format!("manifest.commands[{index}] must be an object"),
            )
        })?;
        if object.get("version").and_then(JsonValue::as_u64) != Some(1) {
            return Err(ClientCompileError::manifest(
                "client.manifest.command_version",
                format!("manifest.commands[{index}].version must be 1"),
            ));
        }
        let name = required_string(
            object.get("name"),
            &format!("manifest.commands[{index}].name"),
        )?;
        let field = required_string(
            object.get("mutation_field"),
            &format!("manifest.commands[{index}].mutation_field"),
        )?;
        if !names.insert(name) {
            return Err(ClientCompileError::manifest(
                "client.manifest.duplicate_command",
                format!("duplicate manifest command `{name}`"),
            ));
        }
        if !fields.insert(field) {
            return Err(ClientCompileError::manifest(
                "client.manifest.duplicate_command_field",
                format!("duplicate manifest command mutation field `{field}`"),
            ));
        }
        let operation = required_string(
            object.get("operation"),
            &format!("manifest.commands[{index}].operation"),
        )?;
        let operation_hash = required_string(
            object.get("operation_hash"),
            &format!("manifest.commands[{index}].operation_hash"),
        )?;
        validate_exact_operation_hash(operation, operation_hash, "command")?;
        validate_framework_operation(
            operation,
            OperationType::Mutation,
            &format!("Client_{field}"),
            field,
            true,
            "command",
        )?;
        let extensions = object
            .get("extensions")
            .and_then(JsonValue::as_object)
            .ok_or_else(|| {
                ClientCompileError::manifest(
                    "client.manifest.command_extensions",
                    format!("manifest command `{name}` must preserve extension slots"),
                )
            })?;
        if extensions.get("version").and_then(JsonValue::as_u64) != Some(2) {
            return Err(ClientCompileError::manifest(
                "client.manifest.command_extensions",
                format!("manifest command `{name}` requires extensions.version 2"),
            ));
        }
        validate_command_consistency(extensions, name)?;
        validate_command_input_defaults(extensions, name)?;
    }
    Ok(())
}

fn validate_command_consistency(
    extensions: &serde_json::Map<String, JsonValue>,
    command: &str,
) -> Result<(), ClientCompileError> {
    let consistency = extensions
        .get("consistency")
        .and_then(JsonValue::as_object)
        .ok_or_else(|| {
            ClientCompileError::manifest(
                "client.manifest.command_consistency",
                format!("manifest command `{command}` requires typed consistency metadata"),
            )
        })?;
    let kind = consistency.get("kind").and_then(JsonValue::as_str);
    if consistency.get("version").and_then(JsonValue::as_u64) != Some(1)
        || !matches!(kind, Some("accepted" | "fact" | "projected"))
    {
        return Err(ClientCompileError::manifest(
            "client.manifest.command_consistency",
            format!(
                "manifest command `{command}` consistency must be version 1 and accepted, fact, or projected"
            ),
        ));
    }
    Ok(())
}

fn validate_command_input_defaults(
    extensions: &serde_json::Map<String, JsonValue>,
    command: &str,
) -> Result<(), ClientCompileError> {
    let Some(defaults) = extensions
        .get("input_defaults")
        .filter(|value| !value.is_null())
    else {
        return Ok(());
    };
    let defaults = defaults.as_object().ok_or_else(|| {
        ClientCompileError::manifest(
            "client.manifest.input_defaults",
            format!("manifest command `{command}` input_defaults must be an object"),
        )
    })?;
    if defaults.get("version").and_then(JsonValue::as_u64) != Some(1) {
        return Err(ClientCompileError::manifest(
            "client.manifest.input_defaults",
            format!("manifest command `{command}` input_defaults.version must be 1"),
        ));
    }
    let entries = defaults
        .get("defaults")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| {
            ClientCompileError::manifest(
                "client.manifest.input_defaults",
                format!("manifest command `{command}` input_defaults.defaults must be an array"),
            )
        })?;
    let mut paths = BTreeSet::new();
    for (index, entry) in entries.iter().enumerate() {
        let entry = entry.as_object().ok_or_else(|| {
            ClientCompileError::manifest(
                "client.manifest.input_defaults",
                format!(
                    "manifest command `{command}` input_defaults.defaults[{index}] must be an object"
                ),
            )
        })?;
        let path = entry
            .get("path")
            .and_then(JsonValue::as_array)
            .filter(|path| path.len() == 1)
            .and_then(|path| path[0].as_str())
            .filter(|field| super::is_graphql_name(field))
            .ok_or_else(|| {
                ClientCompileError::manifest(
                    "client.manifest.input_default_path",
                    format!(
                        "manifest command `{command}` input default {index} must name exactly one top-level GraphQL input field"
                    ),
                )
            })?;
        if !paths.insert(path) {
            return Err(ClientCompileError::manifest(
                "client.manifest.input_default_path",
                format!("manifest command `{command}` repeats input default path `{path}`"),
            ));
        }
        if !matches!(
            entry.get("generator").and_then(JsonValue::as_str),
            Some("uuid_v7" | "ulid")
        ) {
            return Err(ClientCompileError::manifest(
                "client.manifest.input_default_generator",
                format!(
                    "manifest command `{command}` input default `{path}` must use uuid_v7 or ulid"
                ),
            ));
        }
    }
    Ok(())
}

fn validate_protocol_operations(
    operations: &JsonValue,
    commands_present: bool,
) -> Result<(), ClientCompileError> {
    let object = operations.as_object().ok_or_else(|| {
        ClientCompileError::manifest(
            "client.manifest.protocol_operations",
            "manifest.protocol_operations must be an object",
        )
    })?;
    if object.get("version").and_then(JsonValue::as_u64) != Some(1) {
        return Err(ClientCompileError::manifest(
            "client.manifest.protocol_operations",
            "manifest.protocol_operations.version must be 1",
        ));
    }
    let status = object
        .get("command_status")
        .filter(|value| !value.is_null());
    if commands_present && status.is_none() {
        return Err(ClientCompileError::manifest(
            "client.manifest.command_status",
            "manifest commands require the exact framework-owned command_status operation",
        ));
    }
    if let Some(status) = status {
        let status = status.as_object().ok_or_else(|| {
            ClientCompileError::manifest(
                "client.manifest.command_status",
                "manifest.protocol_operations.command_status must be an object",
            )
        })?;
        let operation = required_string(
            status.get("operation"),
            "manifest.protocol_operations.command_status.operation",
        )?;
        let operation_hash = required_string(
            status.get("operation_hash"),
            "manifest.protocol_operations.command_status.operation_hash",
        )?;
        validate_exact_operation_hash(operation, operation_hash, "command status")?;
        let name = required_string(
            status.get("name"),
            "manifest.protocol_operations.command_status.name",
        )?;
        if name != "Distributed_CommandStatus" {
            return Err(ClientCompileError::manifest(
                "client.manifest.command_status",
                "framework command status operation must be named `Distributed_CommandStatus`",
            ));
        }
        validate_framework_operation(
            operation,
            OperationType::Query,
            name,
            "commandStatus",
            true,
            "command status",
        )?;
    }
    Ok(())
}

fn validate_framework_operation(
    source: &str,
    expected_type: OperationType,
    expected_name: &str,
    expected_root: &str,
    require_command_id: bool,
    label: &str,
) -> Result<(), ClientCompileError> {
    let document = async_graphql_parser::parse_query(source).map_err(|error| {
        ClientCompileError::manifest(
            "client.manifest.operation_parse",
            format!("invalid {label} GraphQL operation: {error}"),
        )
    })?;
    if !document.fragments.is_empty() {
        return Err(ClientCompileError::manifest(
            "client.manifest.operation_fragment",
            format!("{label} operation must be self-contained without fragments"),
        ));
    }
    let (operation_name, operation) = match &document.operations {
        DocumentOperations::Single(operation) => (None, operation),
        DocumentOperations::Multiple(operations) if operations.len() == 1 => {
            let (name, operation) = operations.iter().next().expect("length checked");
            (Some(name.as_str()), operation)
        }
        DocumentOperations::Multiple(_) => {
            return Err(ClientCompileError::manifest(
                "client.manifest.operation_count",
                format!("{label} document must contain exactly one operation"),
            ));
        }
    };
    if operation.node.ty != expected_type {
        return Err(ClientCompileError::manifest(
            "client.manifest.operation_type",
            format!("{label} operation has the wrong GraphQL operation type"),
        ));
    }
    if operation_name != Some(expected_name) {
        return Err(ClientCompileError::manifest(
            "client.manifest.operation_name",
            format!("{label} operation must be named `{expected_name}`"),
        ));
    }
    if operation.node.selection_set.node.items.len() != 1 {
        return Err(ClientCompileError::manifest(
            "client.manifest.operation_root_count",
            format!("{label} operation must select exactly one root field"),
        ));
    }
    let field = match &operation.node.selection_set.node.items[0].node {
        Selection::Field(field) if field.node.name.node.as_str() == expected_root => field,
        _ => {
            return Err(ClientCompileError::manifest(
                "client.manifest.operation_root",
                format!("{label} operation must select root `{expected_root}`"),
            ))
        }
    };
    if require_command_id {
        let variable = operation
            .node
            .variable_definitions
            .iter()
            .find(|definition| definition.node.name.node.as_str() == "commandId");
        if variable.is_none_or(|definition| {
            definition.node.var_type.node.to_string() != "ID!"
                || definition.node.default_value.is_some()
        }) {
            return Err(ClientCompileError::manifest(
                "client.manifest.operation_command_id",
                format!("{label} operation must require `$commandId: ID!` without a default"),
            ));
        }
        let forwards_command_id = field.node.arguments.iter().any(|(name, value)| {
            name.node.as_str() == "commandId"
                && matches!(&value.node, Value::Variable(variable) if variable.as_str() == "commandId")
        });
        if !forwards_command_id {
            return Err(ClientCompileError::manifest(
                "client.manifest.operation_command_id",
                format!(
                    "{label} operation root `{expected_root}` must pass `commandId: $commandId`"
                ),
            ));
        }
    }
    Ok(())
}

pub(crate) fn validate_exact_operation_hash(
    operation: &str,
    expected: &str,
    label: &str,
) -> Result<(), ClientCompileError> {
    validate_hash(expected, &format!("{label} operation hash"))?;
    let actual = hash_bytes(operation.as_bytes());
    if actual != expected {
        return Err(ClientCompileError::manifest(
            "client.manifest.operation_hash",
            format!("{label} operation hash mismatch: expected `{expected}`, computed `{actual}`"),
        ));
    }
    Ok(())
}

pub(crate) fn hash_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("sha256:{digest:x}")
}

fn validate_hash(value: &str, label: &str) -> Result<(), ClientCompileError> {
    if value.len() != 71
        || !value.starts_with("sha256:")
        || !value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ClientCompileError::manifest(
            "client.manifest.hash",
            format!("{label} must be a lowercase sha256 fingerprint"),
        ));
    }
    Ok(())
}

fn validate_nonempty(value: &str, label: &str) -> Result<(), ClientCompileError> {
    if value.trim().is_empty() {
        return Err(ClientCompileError::manifest(
            "client.manifest.empty",
            format!("{label} must not be empty"),
        ));
    }
    Ok(())
}

fn required_string<'a>(
    value: Option<&'a JsonValue>,
    label: &str,
) -> Result<&'a str, ClientCompileError> {
    let value = value.and_then(JsonValue::as_str).ok_or_else(|| {
        ClientCompileError::manifest(
            "client.manifest.string",
            format!("{label} must be a string"),
        )
    })?;
    validate_nonempty(value, label)?;
    Ok(value)
}

fn canonical_entries(mut values: Vec<JsonValue>, key: &str) -> Vec<JsonValue> {
    for value in &mut values {
        *value = canonical_json_value(std::mem::take(value));
    }
    values.sort_by(|left, right| {
        left.get(key)
            .and_then(JsonValue::as_str)
            .cmp(&right.get(key).and_then(JsonValue::as_str))
            .then_with(|| left.to_string().cmp(&right.to_string()))
    });
    values
}

pub(crate) fn canonical_json_value(value: JsonValue) -> JsonValue {
    match value {
        JsonValue::Array(values) => {
            JsonValue::Array(values.into_iter().map(canonical_json_value).collect())
        }
        JsonValue::Object(values) => {
            let sorted = values
                .into_iter()
                .map(|(key, value)| (key, canonical_json_value(value)))
                .collect::<BTreeMap<_, _>>();
            JsonValue::Object(sorted.into_iter().collect())
        }
        scalar => scalar,
    }
}
