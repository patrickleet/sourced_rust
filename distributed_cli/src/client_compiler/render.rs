use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::Value as JsonValue;

use super::graphql::{
    typescript_scalar, Cardinality, CompiledArgument, CompiledBranch, CompiledBranchSemantic,
    CompiledFilterField, CompiledFilterInputField, CompiledFilterInputTarget, CompiledFilterPlan,
    CompiledInputDefinition, CompiledInputType, CompiledMember, CompiledObject, CompiledOperation,
    CompiledOrderField, CompiledOrderPlan, CompiledPaginationPlan, CompiledRelationshipPlan,
    CompiledStorage, CompiledVariableCodec,
};
use super::manifest::{
    ClientManifest, ManifestCommand, ManifestCommandShape, ManifestConfirmation,
    ManifestConfirmationKind, ManifestConsistencyKind, ManifestDirectProjection, ManifestEffect,
    ManifestEffectExpression, ManifestEffectField, ManifestEffectKey, ManifestEffectRelationship,
    ManifestEffects, ManifestRelationshipKeyMapping, ManifestRelationshipKind,
    ManifestRelationshipMaintenance, ManifestRevalidationFallback, ManifestRowPolicy,
    ManifestTypeDef, ManifestTypeField,
};
use super::{
    ClientCompileError, GeneratedClientFile, GeneratedClientProject, GeneratedOperationSummary,
    GeneratedRoutePlan,
};

#[derive(Serialize)]
struct Artifact<'a> {
    id: &'a str,
    document: &'a str,
    #[serde(rename = "variableCodec")]
    variable_codec: &'a CompiledVariableCodec,
    roots: Vec<ArtifactRoot<'a>>,
    protocol: ArtifactProtocol<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    live: Option<ArtifactLive<'a>>,
}

#[derive(Serialize)]
struct ArtifactProtocol<'a> {
    version: u32,
    #[serde(rename = "schemaHash")]
    schema_hash: &'a str,
    operation: &'a str,
}

#[derive(Serialize)]
struct ArtifactLive<'a> {
    id: &'a str,
    document: &'a str,
}

#[derive(Serialize)]
struct ArtifactRoot<'a> {
    #[serde(rename = "responseKey")]
    response_key: &'a str,
    field: &'a str,
    cardinality: &'static str,
    nullable: bool,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    arguments: BTreeMap<&'a str, ArtifactArgument<'a>>,
    dependencies: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    coverage: Option<ArtifactCoverage<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    filter: Option<ArtifactFilter<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    order: Option<ArtifactOrder<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pagination: Option<ArtifactPagination<'a>>,
    selection: ArtifactSelection<'a>,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ArtifactArgument<'a> {
    Literal {
        value: &'a JsonValue,
    },
    Variable {
        name: &'a str,
    },
    List {
        items: Vec<ArtifactArgument<'a>>,
    },
    Object {
        fields: BTreeMap<&'a str, ArtifactArgument<'a>>,
    },
}

#[derive(Serialize)]
struct ArtifactCoverage<'a> {
    kind: &'a str,
    #[serde(rename = "offsetArgument", skip_serializing_if = "Option::is_none")]
    offset_argument: Option<&'a str>,
    #[serde(rename = "limitArgument", skip_serializing_if = "Option::is_none")]
    limit_argument: Option<&'a str>,
    #[serde(rename = "defaultLimit", skip_serializing_if = "Option::is_none")]
    default_limit: Option<u64>,
    #[serde(rename = "maxLimit", skip_serializing_if = "Option::is_none")]
    max_limit: Option<u64>,
}

#[derive(Serialize)]
struct ArtifactFilter<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    input: Option<ArtifactArgument<'a>>,
    fields: Vec<ArtifactFilterField<'a>>,
    relationships: Vec<ArtifactRelationship<'a>>,
    #[serde(rename = "rowPolicy")]
    row_policy: &'a ManifestRowPolicy,
}

#[derive(Serialize)]
struct ArtifactFilterField<'a> {
    field: &'a str,
    scalar: &'a str,
    codec: &'a str,
    nullable: bool,
    operators: &'a [String],
}

#[derive(Serialize)]
struct ArtifactRelationship<'a> {
    field: &'a str,
    #[serde(rename = "targetModel")]
    target_model: &'a str,
    kind: ManifestRelationshipKind,
    #[serde(rename = "keyMapping")]
    key_mapping: ArtifactRelationshipKeyMapping<'a>,
    maintenance: ManifestRelationshipMaintenance,
    dependencies: &'a [String],
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ArtifactRelationshipKeyMapping<'a> {
    Direct {
        local: &'a [String],
        remote: &'a [String],
    },
    Through {
        local: &'a [String],
        remote: &'a [String],
        table: &'a str,
        #[serde(rename = "sourceForeignKey")]
        source_foreign_key: &'a str,
        #[serde(rename = "targetForeignKey")]
        target_foreign_key: &'a str,
    },
    ThroughOpaque {
        local: &'a [String],
        remote: &'a [String],
        dependency: &'a str,
    },
    Embedded,
}

#[derive(Serialize)]
struct ArtifactOrder<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    input: Option<ArtifactArgument<'a>>,
    fields: Vec<ArtifactOrderField<'a>>,
    #[serde(rename = "tieBreakers")]
    tie_breakers: Vec<ArtifactOrderField<'a>>,
}

#[derive(Serialize)]
struct ArtifactOrderField<'a> {
    field: &'a str,
    scalar: &'a str,
    codec: &'a str,
    nullable: bool,
}

#[derive(Serialize)]
struct ArtifactPagination<'a> {
    kind: &'a str,
    insert: &'a str,
    delete: &'a str,
    reorder: &'a str,
    #[serde(rename = "stableUpdate")]
    stable_update: &'a str,
}

#[derive(Serialize)]
struct ArtifactSelection<'a> {
    typename: &'a str,
    storage: ArtifactStorage<'a>,
    members: Vec<ArtifactMember<'a>>,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ArtifactStorage<'a> {
    Normalized {
        model: &'a str,
        #[serde(rename = "identityFields")]
        identity_fields: &'a [String],
    },
    Embedded,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ArtifactMember<'a> {
    Scalar {
        #[serde(rename = "responseKey")]
        response_key: &'a str,
        field: &'a str,
        codec: &'a str,
        nullable: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        expose: Option<bool>,
    },
    Branch {
        semantic: CompiledBranchSemantic,
        #[serde(rename = "responseKey")]
        response_key: &'a str,
        field: &'a str,
        cardinality: &'static str,
        nullable: bool,
        #[serde(skip_serializing_if = "BTreeMap::is_empty")]
        arguments: BTreeMap<&'a str, ArtifactArgument<'a>>,
        dependencies: &'a [String],
        #[serde(skip_serializing_if = "Option::is_none")]
        coverage: Option<ArtifactCoverage<'a>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        filter: Option<Box<ArtifactFilter<'a>>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        order: Option<Box<ArtifactOrder<'a>>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pagination: Option<Box<ArtifactPagination<'a>>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        relationship: Option<Box<ArtifactRelationship<'a>>>,
        selection: Box<ArtifactSelection<'a>>,
    },
}

pub(crate) fn render_project(
    manifest: &ClientManifest,
    operations: Vec<CompiledOperation>,
) -> Result<GeneratedClientProject, ClientCompileError> {
    let mut files = Vec::new();
    let mut summaries = Vec::with_capacity(operations.len());
    let mut routes = Vec::new();

    for operation in &operations {
        files.push(GeneratedClientFile {
            path: operation.module_path.clone(),
            contents: render_operation_module(operation, manifest)?,
        });
        summaries.push(GeneratedOperationSummary {
            name: operation.name.clone(),
            source_path: operation.source_path.clone(),
            module_path: operation.module_path.clone(),
            export_name: operation.export_name.clone(),
            operation_hash: operation.query_hash.clone(),
            live_operation_hash: operation.live.as_ref().map(|live| live.hash.clone()),
        });
        if let Some(route) = &operation.route {
            routes.push(route.clone());
        }
    }
    routes.sort_by(|left, right| {
        left.route
            .cmp(&right.route)
            .then_with(|| left.operation.cmp(&right.operation))
    });
    files.push(GeneratedClientFile {
        path: "commands.ts".into(),
        contents: render_commands(manifest)?,
    });
    files.push(GeneratedClientFile {
        path: "protocol.ts".into(),
        contents: render_protocol(manifest)?,
    });
    files.push(GeneratedClientFile {
        path: "routes.ts".into(),
        contents: render_routes(&routes)?,
    });
    files.push(GeneratedClientFile {
        path: "index.ts".into(),
        contents: render_index(&operations),
    });
    files.push(GeneratedClientFile {
        path: "manifest.json".into(),
        contents: render_compiler_manifest(manifest, &summaries, &routes)?,
    });
    files.sort_by(|left, right| left.path.cmp(&right.path));

    Ok(GeneratedClientProject {
        files,
        operations: summaries,
        routes,
        schema_fingerprint: manifest.schema_fingerprint.clone(),
        protocol_fingerprint: manifest.protocol_fingerprint.clone(),
    })
}

pub(super) fn render_operation_module(
    operation: &CompiledOperation,
    manifest: &ClientManifest,
) -> Result<String, ClientCompileError> {
    let variables_name = format!("{}_Variables", operation.export_name);
    let data_name = format!("{}_Data", operation.export_name);
    let variables = render_variables_type(operation, &variables_name)?;
    let data = render_data_type(operation, &data_name)?;
    let artifact_json = render_operation_artifact_json(operation, manifest)?;
    let replica_value_import =
        variable_codec_uses_replica_value(&operation.variable_codec).then_some(", ReplicaValue");
    Ok(format!(
        "/** GENERATED by dctl client. Do not edit. */\n\
         import type {{ ReplicaOperationArtifact{} }} from '@hops-ops/distributed/replica';\n\
         \n\
         {variables}\n\
         \n\
         {data}\n\
         \n\
         /** Exact canonical query bytes sent to the server. */\n\
         export const {}Document = {};\n\
         \n\
         /** Typed normalized-replica operation descriptor. */\n\
         export const {}: ReplicaOperationArtifact<{}, {}> = {};\n",
        replica_value_import.unwrap_or_default(),
        operation.export_name,
        json_string(&operation.query_document)?,
        operation.export_name,
        data_name,
        variables_name,
        artifact_json,
    ))
}

fn variable_codec_uses_replica_value(codec: &CompiledVariableCodec) -> bool {
    fn input_type_uses_replica_value(input: &CompiledInputType) -> bool {
        match input {
            CompiledInputType::Scalar { codec, .. } => codec == "json",
            CompiledInputType::List { item, .. } => input_type_uses_replica_value(item),
            CompiledInputType::Enum { .. } | CompiledInputType::Input { .. } => false,
        }
    }

    codec.variables.values().any(input_type_uses_replica_value)
        || codec.inputs.values().any(|input| match input {
            CompiledInputDefinition::Filter {
                fields,
                relationships,
                ..
            } => {
                fields.iter().any(|field| field.codec == "json")
                    || relationships.iter().any(|relationship| {
                        matches!(&relationship.target, CompiledFilterInputTarget::Opaque)
                    })
            }
            CompiledInputDefinition::Order { .. } => false,
        })
}

/// Serialize the exact artifact embedded in the generated TypeScript module.
///
/// Keeping this boundary machine-readable lets cross-runtime contract tests
/// consume Rust output directly instead of recovering JSON from TypeScript.
pub(super) fn render_operation_artifact_json(
    operation: &CompiledOperation,
    manifest: &ClientManifest,
) -> Result<String, ClientCompileError> {
    let artifact = Artifact {
        id: &operation.query_hash,
        document: &operation.query_document,
        variable_codec: &operation.variable_codec,
        roots: vec![artifact_root(operation)],
        protocol: ArtifactProtocol {
            version: 2,
            schema_hash: &manifest.schema_fingerprint,
            operation: &operation.query_hash,
        },
        live: operation.live.as_ref().map(|live| ArtifactLive {
            id: &live.hash,
            document: &live.document,
        }),
    };
    let artifact_json = serde_json::to_string_pretty(&artifact).map_err(|error| {
        ClientCompileError::manifest(
            "client.render.operation",
            format!("failed to render operation `{}`: {error}", operation.name),
        )
    })?;
    Ok(artifact_json)
}

fn artifact_root(operation: &CompiledOperation) -> ArtifactRoot<'_> {
    let root = &operation.root;
    ArtifactRoot {
        response_key: &root.response_key,
        field: &root.field,
        cardinality: match root.cardinality {
            Cardinality::One => "one",
            Cardinality::Many => "many",
        },
        nullable: root.nullable,
        arguments: root
            .arguments
            .iter()
            .map(|(name, argument)| (name.as_str(), artifact_argument(argument)))
            .collect(),
        dependencies: &root.dependencies,
        coverage: root.coverage.as_ref().map(|coverage| ArtifactCoverage {
            kind: &coverage.kind,
            offset_argument: coverage.offset_argument.as_deref(),
            limit_argument: coverage.limit_argument.as_deref(),
            default_limit: coverage.default_limit,
            max_limit: coverage.max_limit,
        }),
        filter: root.filter.as_ref().map(artifact_filter),
        order: root.order.as_ref().map(artifact_order),
        pagination: root.pagination.as_ref().map(artifact_pagination),
        selection: artifact_selection(&root.selection),
    }
}

fn artifact_argument(argument: &CompiledArgument) -> ArtifactArgument<'_> {
    match argument {
        CompiledArgument::Literal { value, .. } => ArtifactArgument::Literal { value },
        CompiledArgument::Variable(name) => ArtifactArgument::Variable { name },
        CompiledArgument::List(items) => ArtifactArgument::List {
            items: items.iter().map(artifact_argument).collect(),
        },
        CompiledArgument::Object(fields) => ArtifactArgument::Object {
            fields: fields
                .iter()
                .map(|(name, value)| (name.as_str(), artifact_argument(value)))
                .collect(),
        },
    }
}

fn artifact_filter(plan: &CompiledFilterPlan) -> ArtifactFilter<'_> {
    ArtifactFilter {
        input: plan.input.as_ref().map(artifact_argument),
        fields: plan.fields.iter().map(artifact_filter_field).collect(),
        relationships: plan
            .relationships
            .iter()
            .map(artifact_relationship)
            .collect(),
        row_policy: &plan.row_policy,
    }
}

fn artifact_filter_field(field: &CompiledFilterField) -> ArtifactFilterField<'_> {
    ArtifactFilterField {
        field: &field.name,
        scalar: &field.scalar,
        codec: &field.codec,
        nullable: field.nullable,
        operators: &field.operators,
    }
}

fn artifact_relationship(relationship: &CompiledRelationshipPlan) -> ArtifactRelationship<'_> {
    ArtifactRelationship {
        field: &relationship.field,
        target_model: &relationship.target_model,
        kind: relationship.kind,
        key_mapping: match &relationship.key_mapping {
            ManifestRelationshipKeyMapping::Direct { local, remote } => {
                ArtifactRelationshipKeyMapping::Direct { local, remote }
            }
            ManifestRelationshipKeyMapping::Through {
                local,
                remote,
                table,
                source_foreign_key,
                target_foreign_key,
            } => ArtifactRelationshipKeyMapping::Through {
                local,
                remote,
                table,
                source_foreign_key,
                target_foreign_key,
            },
            ManifestRelationshipKeyMapping::ThroughOpaque {
                local,
                remote,
                dependency,
            } => ArtifactRelationshipKeyMapping::ThroughOpaque {
                local,
                remote,
                dependency,
            },
            ManifestRelationshipKeyMapping::Embedded => ArtifactRelationshipKeyMapping::Embedded,
        },
        maintenance: relationship.maintenance,
        dependencies: &relationship.dependencies,
    }
}

fn artifact_order(plan: &CompiledOrderPlan) -> ArtifactOrder<'_> {
    ArtifactOrder {
        input: plan.input.as_ref().map(artifact_argument),
        fields: plan.fields.iter().map(artifact_order_field).collect(),
        tie_breakers: plan.identity.iter().map(artifact_order_field).collect(),
    }
}

fn artifact_order_field(field: &CompiledOrderField) -> ArtifactOrderField<'_> {
    ArtifactOrderField {
        field: &field.name,
        scalar: &field.scalar,
        codec: &field.codec,
        nullable: field.nullable,
    }
}

fn artifact_pagination(plan: &CompiledPaginationPlan) -> ArtifactPagination<'_> {
    ArtifactPagination {
        kind: &plan.kind,
        insert: &plan.insert,
        delete: &plan.delete,
        reorder: &plan.reorder,
        stable_update: &plan.stable_update,
    }
}

fn artifact_selection(selection: &CompiledObject) -> ArtifactSelection<'_> {
    ArtifactSelection {
        typename: &selection.typename,
        storage: match &selection.storage {
            CompiledStorage::Normalized {
                model_id,
                identity_fields,
            } => ArtifactStorage::Normalized {
                model: model_id,
                identity_fields,
            },
            CompiledStorage::Embedded => ArtifactStorage::Embedded,
        },
        members: selection
            .members
            .iter()
            .map(|member| match member {
                CompiledMember::Scalar(scalar) => ArtifactMember::Scalar {
                    response_key: &scalar.response_key,
                    field: &scalar.field,
                    codec: &scalar.codec,
                    nullable: scalar.nullable,
                    expose: (!scalar.expose).then_some(false),
                },
                CompiledMember::Branch(branch) => artifact_branch(branch),
            })
            .collect(),
    }
}

fn artifact_branch(branch: &CompiledBranch) -> ArtifactMember<'_> {
    ArtifactMember::Branch {
        semantic: branch.semantic,
        response_key: &branch.response_key,
        field: &branch.field,
        cardinality: match branch.cardinality {
            Cardinality::One => "one",
            Cardinality::Many => "many",
        },
        nullable: branch.nullable,
        arguments: branch
            .arguments
            .iter()
            .map(|(name, argument)| (name.as_str(), artifact_argument(argument)))
            .collect(),
        dependencies: &branch.dependencies,
        coverage: branch.coverage.as_ref().map(|coverage| ArtifactCoverage {
            kind: &coverage.kind,
            offset_argument: coverage.offset_argument.as_deref(),
            limit_argument: coverage.limit_argument.as_deref(),
            default_limit: coverage.default_limit,
            max_limit: coverage.max_limit,
        }),
        filter: branch
            .filter
            .as_ref()
            .map(|plan| Box::new(artifact_filter(plan))),
        order: branch
            .order
            .as_ref()
            .map(|plan| Box::new(artifact_order(plan))),
        pagination: branch
            .pagination
            .as_ref()
            .map(|plan| Box::new(artifact_pagination(plan))),
        relationship: branch
            .relationship
            .as_ref()
            .map(|relationship| Box::new(artifact_relationship(relationship))),
        selection: Box::new(artifact_selection(&branch.selection)),
    }
}

fn render_variables_type(
    operation: &CompiledOperation,
    name: &str,
) -> Result<String, ClientCompileError> {
    if operation.variables.is_empty() {
        return Ok(format!("export type {name} = Record<string, never>;"));
    }
    let mut blocks = operation
        .variable_codec
        .inputs
        .iter()
        .map(|(input_name, definition)| {
            render_input_definition(&operation.export_name, input_name, definition)
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut lines = vec![format!("export type {name} = {{")];
    for variable in &operation.variables {
        let input_type = operation
            .variable_codec
            .variables
            .get(&variable.name)
            .ok_or_else(|| {
                ClientCompileError::manifest(
                    "client.render.variable_codec",
                    format!(
                        "operation `{}` has no codec for variable `${}`",
                        operation.name, variable.name
                    ),
                )
            })?;
        lines.push(format!(
            "  readonly {}{}: {};",
            quoted_property(&variable.name),
            if variable.graphql_type.nullable {
                "?"
            } else {
                ""
            },
            render_input_type(&operation.export_name, input_type)?
        ));
    }
    lines.push("};".into());
    blocks.push(lines.join("\n"));
    Ok(blocks.join("\n\n"))
}

fn render_input_definition(
    operation: &str,
    name: &str,
    definition: &CompiledInputDefinition,
) -> Result<String, ClientCompileError> {
    let alias = input_alias(operation, name);
    match definition {
        CompiledInputDefinition::Filter {
            fields,
            relationships,
            ..
        } => {
            let mut lines = vec![format!("type {alias} = {{")];
            for operator in ["_and", "_or"] {
                lines.push(format!(
                    "  readonly {}?: {alias} | readonly {alias}[] | null;",
                    quoted_property(operator)
                ));
            }
            lines.push(format!(
                "  readonly {}?: {alias} | null;",
                quoted_property("_not")
            ));
            for field in fields {
                lines.push(format!(
                    "  readonly {}?: {};",
                    quoted_property(&field.field),
                    render_filter_comparison(field)?
                ));
            }
            for relationship in relationships {
                let target = match &relationship.target {
                    CompiledFilterInputTarget::Input { name } => input_alias(operation, name),
                    CompiledFilterInputTarget::Opaque => {
                        "{ readonly [key: string]: ReplicaValue }".into()
                    }
                };
                lines.push(format!(
                    "  readonly {}?: {target} | null;",
                    quoted_property(&relationship.field)
                ));
            }
            lines.push("};".into());
            Ok(lines.join("\n"))
        }
        CompiledInputDefinition::Order { fields, values, .. } => {
            let direction = format!("{alias}_Direction");
            let direction_type = render_string_union(values)?;
            let mut lines = vec![format!("type {direction} = {direction_type};")];
            if fields.is_empty() {
                lines.push(format!("type {alias} = never;"));
            } else {
                lines.push(format!("type {alias} ="));
                for (index, field) in fields.iter().enumerate() {
                    lines.push("  | {".into());
                    for candidate in fields {
                        let property = quoted_property(&candidate.field);
                        if candidate.field == field.field {
                            lines.push(format!("      readonly {property}: {direction};"));
                        } else {
                            lines.push(format!("      readonly {property}?: never;"));
                        }
                    }
                    lines.push(if index + 1 == fields.len() {
                        "    };".into()
                    } else {
                        "    }".into()
                    });
                }
            }
            Ok(lines.join("\n"))
        }
    }
}

fn render_filter_comparison(
    field: &CompiledFilterInputField,
) -> Result<String, ClientCompileError> {
    let scalar = typescript_input_scalar(&field.scalar, &field.codec)?;
    let mut lines = vec!["{".to_string()];
    for operator in &field.operators {
        let value = match operator.as_str() {
            "_is_null" => "boolean | null".into(),
            "_in" | "_nin" => format!("{scalar} | readonly {}[]", parenthesize_ts_union(scalar)),
            "_has_key" => "string | null".into(),
            _ => format!("{scalar} | null"),
        };
        lines.push(format!(
            "    readonly {}?: {value};",
            quoted_property(operator)
        ));
    }
    lines.push("  } | null".into());
    Ok(lines.join("\n"))
}

fn render_input_type(
    operation: &str,
    input_type: &CompiledInputType,
) -> Result<String, ClientCompileError> {
    let (base, nullable) = match input_type {
        CompiledInputType::Scalar {
            scalar,
            codec,
            nullable,
        } => (
            typescript_input_scalar(scalar, codec)?.to_string(),
            *nullable,
        ),
        CompiledInputType::Enum {
            values, nullable, ..
        } => (render_string_union(values)?, *nullable),
        CompiledInputType::Input { name, nullable, .. } => {
            (input_alias(operation, name), *nullable)
        }
        CompiledInputType::List { nullable, item, .. } => {
            let item = render_input_type(operation, item)?;
            (
                format!("{item} | readonly {}[]", parenthesize_ts_union(&item)),
                *nullable,
            )
        }
    };
    Ok(if nullable {
        format!("{base} | null")
    } else {
        base
    })
}

fn typescript_input_scalar(scalar: &str, codec: &str) -> Result<&'static str, ClientCompileError> {
    match (scalar, codec) {
        ("ID", "string") => Ok("string | number"),
        ("String", "string")
        | ("Bytea", "base64")
        | ("Timestamptz", "string_unvalidated_timestamp") => Ok("string"),
        ("Boolean", "boolean") => Ok("boolean"),
        ("Int", "int32") | ("Float", "float64") | ("BigInt", "json_number_precision_limited") => {
            Ok("number")
        }
        ("JSON", "json") => Ok("ReplicaValue"),
        _ => Err(ClientCompileError::manifest(
            "client.scalar.codec_unsupported",
            format!("scalar `{scalar}` uses unsupported input codec `{codec}`"),
        )),
    }
}

fn render_string_union(values: &[String]) -> Result<String, ClientCompileError> {
    if values.is_empty() {
        return Err(ClientCompileError::manifest(
            "client.render.input_enum",
            "generated input enum must contain at least one value",
        ));
    }
    values
        .iter()
        .map(|value| json_string(value))
        .collect::<Result<Vec<_>, _>>()
        .map(|values| values.join(" | "))
}

fn input_alias(operation: &str, input: &str) -> String {
    format!("{operation}_Input_{input}")
}

fn parenthesize_ts_union(value: &str) -> String {
    if value.contains(" | ") {
        format!("({value})")
    } else {
        value.to_string()
    }
}

fn render_data_type(
    operation: &CompiledOperation,
    name: &str,
) -> Result<String, ClientCompileError> {
    let root = &operation.root;
    let entity = render_object_type(&root.selection, 2)?;
    let value = match root.cardinality {
        Cardinality::Many => format!("readonly {entity}[]"),
        Cardinality::One => {
            if root.nullable {
                format!("{entity} | null")
            } else {
                entity
            }
        }
    };
    Ok(format!(
        "export type {name} = {{\n  readonly {}: {};\n}};",
        quoted_property(&root.response_key),
        value
    ))
}

fn render_object_type(
    object: &CompiledObject,
    indent: usize,
) -> Result<String, ClientCompileError> {
    let member_padding = " ".repeat(indent + 2);
    let closing_padding = " ".repeat(indent);
    let mut lines = vec!["{".to_string()];
    for member in &object.members {
        match member {
            CompiledMember::Scalar(field) if field.expose => {
                let scalar = typescript_scalar(field)?;
                lines.push(format!(
                    "{member_padding}readonly {}: {}{};",
                    quoted_property(&field.response_key),
                    scalar,
                    if field.nullable { " | null" } else { "" }
                ));
            }
            CompiledMember::Scalar(_) => {}
            CompiledMember::Branch(branch) => {
                let object = render_object_type(&branch.selection, indent + 2)?;
                let value = match branch.cardinality {
                    Cardinality::Many => format!("readonly {object}[]"),
                    Cardinality::One if branch.nullable => format!("{object} | null"),
                    Cardinality::One => object,
                };
                lines.push(format!(
                    "{member_padding}readonly {}: {value};",
                    quoted_property(&branch.response_key)
                ));
            }
        }
    }
    lines.push(format!("{closing_padding}}}"));
    Ok(lines.join("\n"))
}

fn render_commands(manifest: &ClientManifest) -> Result<String, ClientCompileError> {
    let projectors = serde_json::to_string_pretty(&manifest.projectors).map_err(|error| {
        ClientCompileError::manifest(
            "client.render.projectors",
            format!("failed to render projector artifacts: {error}"),
        )
    })?;
    let mut sections = vec![
        "/** GENERATED by dctl client. Do not edit. */".to_string(),
        "import { prepareReplicaCommand } from '@hops-ops/distributed/replica';".into(),
        "import type {\n  PrepareReplicaCommandOptions,\n  ReplicaCommandArtifact,\n  ReplicaPreparedCommand,\n  ReplicaValue\n} from '@hops-ops/distributed/replica';".into(),
    ];
    for command in &manifest.commands {
        sections.push(render_command(command, manifest)?);
    }
    let artifact_names = manifest
        .commands
        .iter()
        .map(|command| format!("Command_{}", command.mutation_field))
        .collect::<Vec<_>>()
        .join(", ");
    sections.push(format!(
        "export const COMMAND_ARTIFACTS = [{artifact_names}] as const;"
    ));
    let command_entries = manifest
        .commands
        .iter()
        .map(|command| {
            format!(
                "  {}: {{ artifact: Command_{}, prepare: prepareCommand_{} }}",
                quoted_property(&command.name),
                command.mutation_field,
                command.mutation_field
            )
        })
        .collect::<Vec<_>>()
        .join(",\n");
    sections.push(format!(
        "/** Typed framework-neutral command entrypoints. */\nexport const commands = {{\n{command_entries}\n}} as const;"
    ));
    sections.push(format!(
        "/** Projector topology used by command confirmation/effect runtimes. */\nexport const PROJECTOR_ARTIFACTS = {projectors} as const;"
    ));
    sections
        .push("export type GeneratedCommandArtifact = (typeof COMMAND_ARTIFACTS)[number];".into());
    Ok(format!("{}\n", sections.join("\n\n")))
}

fn render_command(
    command: &ManifestCommand,
    manifest: &ClientManifest,
) -> Result<String, ClientCompileError> {
    // Domain command names deliberately permit dots and other non-GraphQL
    // characters. Generated identifiers use the unique, GraphQL-validated
    // mutation field; the public command map retains the exact domain name.
    let identifier = &command.mutation_field;
    let input_name = format!("Command_{identifier}_Input");
    let output_name = format!("Command_{identifier}_Output");
    let defaults = command
        .extensions
        .input_defaults
        .as_ref()
        .map(|defaults| {
            defaults
                .defaults
                .iter()
                .map(|default| default.path.clone())
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    let input_type = render_command_shape_type(&command.input, true, &defaults)?;
    let output_type = render_command_shape_type(&command.output, false, &BTreeSet::new())?;
    let artifact = command_artifact_json(command, manifest)?;
    let artifact = serde_json::to_string_pretty(&artifact).map_err(|error| {
        ClientCompileError::manifest(
            "client.render.command",
            format!(
                "failed to render executable command `{}`: {error}",
                command.name
            ),
        )
    })?;
    let prepare = match command.input {
        ManifestCommandShape::None => format!(
            "export function prepareCommand_{}(\n  options?: PrepareReplicaCommandOptions\n): ReplicaPreparedCommand<{}, {}> {{\n  return prepareReplicaCommand(Command_{}, undefined, options);\n}}",
            identifier, input_name, output_name, identifier
        ),
        _ => format!(
            "export function prepareCommand_{}(\n  input: {},\n  options?: PrepareReplicaCommandOptions\n): ReplicaPreparedCommand<{}, {}> {{\n  return prepareReplicaCommand(Command_{}, input, options);\n}}",
            identifier, input_name, input_name, output_name, identifier
        ),
    };
    Ok(format!(
        "export type {input_name} = {input_type};\n\n\
         export type {output_name} = {output_type};\n\n\
         /** Exact typed causal command descriptor and full mutation bytes. */\n\
         export const Command_{}: ReplicaCommandArtifact<{}, {}> = {};\n\n\
         {}",
        identifier, input_name, output_name, artifact, prepare
    ))
}

fn render_command_shape_type(
    shape: &ManifestCommandShape,
    input: bool,
    defaults: &BTreeSet<Vec<String>>,
) -> Result<String, ClientCompileError> {
    match shape {
        ManifestCommandShape::None => Ok("void".into()),
        ManifestCommandShape::Json { .. } => Ok("ReplicaValue".into()),
        ManifestCommandShape::Object { definition } => {
            render_command_type_definition(definition, input, defaults, &[], 0)
        }
    }
}

fn render_command_type_definition(
    definition: &ManifestTypeDef,
    input: bool,
    defaults: &BTreeSet<Vec<String>>,
    prefix: &[String],
    indent: usize,
) -> Result<String, ClientCompileError> {
    let member_padding = " ".repeat(indent + 2);
    let closing_padding = " ".repeat(indent);
    let mut lines = vec!["{".to_string()];
    for field in &definition.fields {
        let mut path = prefix.to_vec();
        path.push(field.name.clone());
        let optional = input && (field.nullable || defaults.contains(&path));
        let mut value = render_command_field_type(field, input, defaults, &path, indent + 2)?;
        if field.list {
            if field.item_nullable {
                value = format!("({value} | null)");
            }
            value = format!("readonly {value}[]");
        }
        if field.nullable {
            value = format!("{value} | null");
        }
        lines.push(format!(
            "{member_padding}readonly {}{}: {value};",
            quoted_property(&field.name),
            if optional { "?" } else { "" }
        ));
    }
    lines.push(format!("{closing_padding}}}"));
    Ok(lines.join("\n"))
}

fn render_command_field_type(
    field: &ManifestTypeField,
    input: bool,
    defaults: &BTreeSet<Vec<String>>,
    path: &[String],
    indent: usize,
) -> Result<String, ClientCompileError> {
    if let Some(nested) = &field.nested {
        return render_command_type_definition(nested, input, defaults, path, indent);
    }
    match field.codec.as_deref() {
        Some("boolean") => Ok("boolean".into()),
        Some("float64" | "int32" | "json_number_precision_limited") => Ok("number".into()),
        Some("string" | "base64" | "string_unvalidated_timestamp") => Ok("string".into()),
        Some("json") => Ok("ReplicaValue".into()),
        Some(codec) => Err(ClientCompileError::manifest(
            "client.scalar.codec_unsupported",
            format!(
                "command field `{}` uses unsupported TypeScript codec `{codec}`",
                field.name
            ),
        )),
        None => Err(ClientCompileError::manifest(
            "client.render.command_shape",
            format!(
                "command field `{}` has neither a scalar codec nor a nested definition",
                field.name
            ),
        )),
    }
}

fn command_artifact_json(
    command: &ManifestCommand,
    manifest: &ClientManifest,
) -> Result<JsonValue, ClientCompileError> {
    let consistency = command.extensions.consistency.as_ref().ok_or_else(|| {
        ClientCompileError::manifest(
            "client.render.command_consistency",
            format!("command `{}` has no validated consistency", command.name),
        )
    })?;
    let mut artifact = serde_json::Map::new();
    artifact.insert("version".into(), serde_json::json!(command.version));
    artifact.insert("name".into(), serde_json::json!(command.name));
    artifact.insert(
        "mutationField".into(),
        serde_json::json!(command.mutation_field),
    );
    artifact.insert("document".into(), serde_json::json!(command.operation));
    artifact.insert(
        "operationHash".into(),
        serde_json::json!(command.operation_hash),
    );
    artifact.insert(
        "protocol".into(),
        serde_json::json!({
            "version": 2,
            "schemaHash": manifest.schema_fingerprint,
            "protocolHash": manifest.protocol_fingerprint,
            "operation": command.operation_hash,
        }),
    );
    artifact.insert("input".into(), command_shape_json(&command.input));
    artifact.insert("output".into(), command_shape_json(&command.output));
    if let Some(defaults) = &command.extensions.input_defaults {
        artifact.insert(
            "inputDefaults".into(),
            serde_json::json!({
                "version": defaults.version,
                "defaults": defaults.defaults,
            }),
        );
    }
    artifact.insert(
        "consistency".into(),
        serde_json::json!(consistency_label(consistency.kind)),
    );
    artifact.insert(
        "effects".into(),
        effects_json(command.extensions.effects.as_ref()),
    );
    if let Some(confirmations) = &command.extensions.confirmations {
        artifact.insert(
            "confirmations".into(),
            serde_json::json!({
                "version": confirmations.version,
                "kind": confirmation_kind_label(confirmations.kind),
                "expected": confirmations.expected.iter().map(confirmation_json).collect::<Vec<_>>(),
                "fallback": "revalidate",
            }),
        );
    }
    if let Some(direct) = &command.extensions.direct_projection {
        artifact.insert(
            "directProjection".into(),
            direct_projection_json(direct, manifest)?,
        );
    }
    artifact.insert(
        "revalidation".into(),
        command_revalidation_json(command, manifest),
    );
    Ok(JsonValue::Object(artifact))
}

fn command_shape_json(shape: &ManifestCommandShape) -> JsonValue {
    match shape {
        ManifestCommandShape::None => serde_json::json!({"kind": "none"}),
        ManifestCommandShape::Json { codec } => {
            serde_json::json!({"kind": "json", "codec": codec})
        }
        ManifestCommandShape::Object { definition } => serde_json::json!({
            "kind": "object",
            "definition": command_type_definition_json(definition),
        }),
    }
}

fn command_type_definition_json(definition: &ManifestTypeDef) -> JsonValue {
    serde_json::json!({
        "name": definition.name,
        "fields": definition.fields.iter().map(command_type_field_json).collect::<Vec<_>>(),
    })
}

fn command_type_field_json(field: &ManifestTypeField) -> JsonValue {
    let mut result = serde_json::Map::new();
    result.insert("name".into(), serde_json::json!(field.name));
    result.insert("typeName".into(), serde_json::json!(field.type_name));
    result.insert("nullable".into(), serde_json::json!(field.nullable));
    result.insert("list".into(), serde_json::json!(field.list));
    result.insert(
        "itemNullable".into(),
        serde_json::json!(field.item_nullable),
    );
    if let Some(codec) = &field.codec {
        result.insert("codec".into(), serde_json::json!(codec));
    }
    if let Some(nested) = &field.nested {
        result.insert("nested".into(), command_type_definition_json(nested));
    }
    JsonValue::Object(result)
}

fn consistency_label(kind: ManifestConsistencyKind) -> &'static str {
    match kind {
        ManifestConsistencyKind::Accepted => "accepted",
        ManifestConsistencyKind::Fact => "fact",
        ManifestConsistencyKind::Projected => "projected",
    }
}

fn confirmation_kind_label(kind: ManifestConfirmationKind) -> &'static str {
    match kind {
        ManifestConfirmationKind::Finite => "finite",
        ManifestConfirmationKind::Unavailable => "unavailable",
    }
}

fn effects_json(effects: Option<&ManifestEffects>) -> JsonValue {
    match effects {
        Some(effects) => serde_json::json!({
            "version": effects.version,
            "operations": effects.operations.iter().map(effect_json).collect::<Vec<_>>(),
            "fallback": revalidation_label(effects.fallback),
        }),
        None => serde_json::json!({
            "version": 1,
            "operations": [],
            "fallback": "revalidate",
        }),
    }
}

fn revalidation_label(fallback: ManifestRevalidationFallback) -> &'static str {
    match fallback {
        ManifestRevalidationFallback::Revalidate => "revalidate",
    }
}

fn effect_json(effect: &ManifestEffect) -> JsonValue {
    match effect {
        ManifestEffect::Upsert { model, key, fields } => serde_json::json!({
            "kind": "upsert",
            "model": model,
            "key": effect_key_json(key),
            "fields": fields.iter().map(effect_field_json).collect::<Vec<_>>(),
        }),
        ManifestEffect::Patch { model, key, fields } => serde_json::json!({
            "kind": "patch",
            "model": model,
            "key": effect_key_json(key),
            "fields": fields.iter().map(effect_field_json).collect::<Vec<_>>(),
        }),
        ManifestEffect::Delete { model, key } => serde_json::json!({
            "kind": "delete",
            "model": model,
            "key": effect_key_json(key),
        }),
        ManifestEffect::Link {
            relationship,
            source,
            target,
        } => serde_json::json!({
            "kind": "link",
            "relationship": effect_relationship_json(relationship),
            "source": effect_key_json(source),
            "target": effect_key_json(target),
        }),
        ManifestEffect::Unlink {
            relationship,
            source,
            target,
        } => serde_json::json!({
            "kind": "unlink",
            "relationship": effect_relationship_json(relationship),
            "source": effect_key_json(source),
            "target": effect_key_json(target),
        }),
        ManifestEffect::InvalidateModel { model } => serde_json::json!({
            "kind": "invalidate_model",
            "model": model,
        }),
        ManifestEffect::InvalidateRelationship {
            relationship,
            source,
        } => serde_json::json!({
            "kind": "invalidate_relationship",
            "relationship": effect_relationship_json(relationship),
            "source": effect_key_json(source),
        }),
    }
}

fn effect_relationship_json(relationship: &ManifestEffectRelationship) -> JsonValue {
    serde_json::json!({
        "sourceModel": relationship.source_model,
        "field": relationship.field,
        "targetModel": relationship.target_model,
    })
}

fn effect_key_json(key: &ManifestEffectKey) -> JsonValue {
    serde_json::json!({
        "fields": key.fields.iter().map(effect_field_json).collect::<Vec<_>>(),
    })
}

fn effect_field_json(field: &ManifestEffectField) -> JsonValue {
    serde_json::json!({
        "field": field.field,
        "value": effect_expression_json(&field.value),
    })
}

fn effect_expression_json(expression: &ManifestEffectExpression) -> JsonValue {
    match expression {
        ManifestEffectExpression::Input { path } => {
            serde_json::json!({"kind": "input", "path": path})
        }
        ManifestEffectExpression::TrustedPreset { name } => {
            serde_json::json!({"kind": "trusted_preset", "name": name})
        }
        ManifestEffectExpression::Constant { value } => {
            serde_json::json!({"kind": "constant", "value": value})
        }
        ManifestEffectExpression::Null => serde_json::json!({"kind": "null"}),
    }
}

fn confirmation_json(confirmation: &ManifestConfirmation) -> JsonValue {
    let mut result = serde_json::Map::new();
    result.insert(
        "projector".into(),
        serde_json::json!(confirmation.projector),
    );
    result.insert("model".into(), serde_json::json!(confirmation.model));
    result.insert("key".into(), effect_key_json(&confirmation.key));
    if let Some(partition) = &confirmation.partition {
        result.insert("partition".into(), effect_expression_json(partition));
    }
    JsonValue::Object(result)
}

fn direct_projection_json(
    direct: &ManifestDirectProjection,
    manifest: &ClientManifest,
) -> Result<JsonValue, ClientCompileError> {
    let identity = manifest
        .models
        .get(&direct.model)
        .and_then(|model| model.identity())
        .filter(|fields| !fields.is_empty())
        .ok_or_else(|| {
            ClientCompileError::manifest(
                "client.render.direct_projection_identity",
                format!(
                    "direct projection model `{}` has no complete normalized identity",
                    direct.model
                ),
            )
        })?;
    let mut result = serde_json::Map::new();
    result.insert(
        "topology".into(),
        serde_json::json!({
            "version": direct.topology.version,
            "name": direct.topology.name,
            "digest": direct.topology.digest,
        }),
    );
    result.insert("model".into(), serde_json::json!(direct.model));
    result.insert(
        "identityFields".into(),
        serde_json::json!(identity
            .iter()
            .map(|field| field.name.as_str())
            .collect::<Vec<_>>()),
    );
    if let Some(partition) = &direct.partition {
        result.insert("partition".into(), effect_expression_json(partition));
    }
    result.insert("changeEpoch".into(), serde_json::json!(direct.change_epoch));
    Ok(JsonValue::Object(result))
}

fn command_revalidation_json(command: &ManifestCommand, manifest: &ClientManifest) -> JsonValue {
    let required = manifest
        .commands_requiring_revalidation
        .contains(&command.name);
    let mut models = BTreeSet::new();
    let mut relationships = BTreeSet::new();
    let mut dependencies = BTreeSet::new();
    if let Some(effects) = &command.extensions.effects {
        for effect in &effects.operations {
            collect_effect_scope(effect, &mut models, &mut relationships);
        }
    }
    if let Some(confirmations) = &command.extensions.confirmations {
        for confirmation in &confirmations.expected {
            models.insert(confirmation.model.clone());
            if let Some(projector) = manifest
                .projectors
                .iter()
                .find(|projector| projector.name == confirmation.projector)
            {
                dependencies.extend(projector.dependencies.iter().cloned());
            }
        }
    }
    if let Some(direct) = &command.extensions.direct_projection {
        models.insert(direct.model.clone());
        if let Some(projector) = manifest
            .projectors
            .iter()
            .find(|projector| projector.name == direct.topology.name)
        {
            dependencies.extend(projector.dependencies.iter().cloned());
        }
    }
    if required && models.is_empty() {
        models.extend(manifest.models.keys().cloned());
    }
    for model in &models {
        if let Some(model) = manifest.models.get(model) {
            dependencies.extend(model.dependencies.iter().cloned());
        }
    }
    let relationship_values = relationships
        .into_iter()
        .map(|(source_model, field, target_model)| {
            serde_json::json!({
                "sourceModel": source_model,
                "field": field,
                "targetModel": target_model,
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "version": 1,
        "required": required,
        "dependencies": dependencies.into_iter().collect::<Vec<_>>(),
        "models": models.into_iter().collect::<Vec<_>>(),
        "relationships": relationship_values,
    })
}

fn collect_effect_scope(
    effect: &ManifestEffect,
    models: &mut BTreeSet<String>,
    relationships: &mut BTreeSet<(String, String, String)>,
) {
    match effect {
        ManifestEffect::Upsert { model, .. }
        | ManifestEffect::Patch { model, .. }
        | ManifestEffect::Delete { model, .. }
        | ManifestEffect::InvalidateModel { model } => {
            models.insert(model.clone());
        }
        ManifestEffect::Link { relationship, .. }
        | ManifestEffect::Unlink { relationship, .. }
        | ManifestEffect::InvalidateRelationship { relationship, .. } => {
            models.insert(relationship.source_model.clone());
            models.insert(relationship.target_model.clone());
            relationships.insert((
                relationship.source_model.clone(),
                relationship.field.clone(),
                relationship.target_model.clone(),
            ));
        }
    }
}

fn render_protocol(manifest: &ClientManifest) -> Result<String, ClientCompileError> {
    let operations =
        serde_json::to_string_pretty(&manifest.protocol_operations).map_err(|error| {
            ClientCompileError::manifest(
                "client.render.protocol",
                format!("failed to render protocol artifacts: {error}"),
            )
        })?;
    Ok(format!(
        "/** GENERATED by dctl client. Exact framework-owned operation bytes. */\n\
         export const CLIENT_PROTOCOL = {{\n\
         \tversion: 2,\n\
         \tserviceId: {},\n\
         \tschemaHash: {},\n\
         \tprotocolHash: {},\n\
         \toperations: {operations}\n\
         }} as const;\n",
        json_string(&manifest.service_id)?,
        json_string(&manifest.schema_fingerprint)?,
        json_string(&manifest.protocol_fingerprint)?,
    ))
}

#[derive(Serialize)]
struct CompilerManifest<'a> {
    compiler_manifest_version: u32,
    distributed_manifest_version: u32,
    protocol_version: u32,
    service_id: &'a str,
    surface: &'a super::manifest::ManifestSurface,
    schema_fingerprint: &'a str,
    protocol_fingerprint: &'a str,
    scalar_codecs: &'a BTreeMap<String, String>,
    commands_requiring_revalidation: &'a BTreeSet<String>,
    operations: &'a [GeneratedOperationSummary],
    routes: &'a [GeneratedRoutePlan],
}

fn render_compiler_manifest(
    manifest: &ClientManifest,
    operations: &[GeneratedOperationSummary],
    routes: &[GeneratedRoutePlan],
) -> Result<String, ClientCompileError> {
    let provenance = CompilerManifest {
        compiler_manifest_version: 1,
        distributed_manifest_version: 6,
        protocol_version: 2,
        service_id: &manifest.service_id,
        surface: &manifest.surface,
        schema_fingerprint: &manifest.schema_fingerprint,
        protocol_fingerprint: &manifest.protocol_fingerprint,
        scalar_codecs: &manifest.scalar_codecs,
        commands_requiring_revalidation: &manifest.commands_requiring_revalidation,
        operations,
        routes,
    };
    serde_json::to_string_pretty(&provenance)
        .map(|rendered| format!("{rendered}\n"))
        .map_err(|error| {
            ClientCompileError::manifest(
                "client.render.manifest",
                format!("failed to render compiler provenance manifest: {error}"),
            )
        })
}

fn render_routes(routes: &[GeneratedRoutePlan]) -> Result<String, ClientCompileError> {
    let routes = serde_json::to_string_pretty(routes).map_err(|error| {
        ClientCompileError::manifest(
            "client.render.routes",
            format!("failed to render route plan: {error}"),
        )
    })?;
    Ok(format!(
        "/** GENERATED framework-neutral `@load` ownership plan. */\n\
         export const DISTRIBUTED_ROUTES = {routes} as const;\n\
         \n\
         export type DistributedRoutePlan = (typeof DISTRIBUTED_ROUTES)[number];\n"
    ))
}

fn render_index(operations: &[CompiledOperation]) -> String {
    let mut lines = vec![
        "/** GENERATED public entrypoint. */".to_string(),
        "export * from './commands.js';".into(),
        "export * from './protocol.js';".into(),
        "export * from './routes.js';".into(),
    ];
    for operation in operations {
        let module = operation
            .module_path
            .strip_suffix(".ts")
            .expect("compiler module paths end in .ts");
        lines.push(format!("export * from './{module}.js';"));
    }
    format!("{}\n", lines.join("\n"))
}

fn quoted_property(value: &str) -> String {
    // GraphQL response keys are valid identifiers, but quoting them prevents
    // TypeScript keyword collisions without inventing a second public name.
    serde_json::to_string(value).expect("string serialization cannot fail")
}

fn json_string(value: &str) -> Result<String, ClientCompileError> {
    serde_json::to_string(value).map_err(|error| {
        ClientCompileError::manifest(
            "client.render.string",
            format!("failed to render generated string literal: {error}"),
        )
    })
}
