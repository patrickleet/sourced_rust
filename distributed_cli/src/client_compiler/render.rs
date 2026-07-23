use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::Value as JsonValue;

use super::graphql::{
    typescript_scalar, typescript_type, Cardinality, CompiledArgument, CompiledBranch,
    CompiledBranchSemantic, CompiledMember, CompiledObject, CompiledOperation, CompiledStorage,
};
use super::manifest::ClientManifest;
use super::{
    ClientCompileError, GeneratedClientFile, GeneratedClientProject, GeneratedOperationSummary,
    GeneratedRoutePlan,
};

#[derive(Serialize)]
struct Artifact<'a> {
    id: &'a str,
    document: &'a str,
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
    selection: ArtifactSelection<'a>,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ArtifactArgument<'a> {
    Literal { value: &'a JsonValue },
    Variable { name: &'a str },
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
        maintenance: Option<&'a str>,
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

fn render_operation_module(
    operation: &CompiledOperation,
    manifest: &ClientManifest,
) -> Result<String, ClientCompileError> {
    let variables_name = format!("{}_Variables", operation.export_name);
    let data_name = format!("{}_Data", operation.export_name);
    let variables = render_variables_type(operation, &variables_name);
    let data = render_data_type(operation, &data_name)?;
    let artifact = Artifact {
        id: &operation.query_hash,
        document: &operation.query_document,
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
    Ok(format!(
        "/** GENERATED by dctl client. Do not edit. */\n\
         import type {{ ReplicaOperationArtifact }} from '@hops-ops/distributed/replica';\n\
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
        operation.export_name,
        json_string(&operation.query_document)?,
        operation.export_name,
        data_name,
        variables_name,
        artifact_json,
    ))
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
            .map(|(name, argument)| {
                (
                    name.as_str(),
                    match argument {
                        CompiledArgument::Literal { value, .. } => {
                            ArtifactArgument::Literal { value }
                        }
                        CompiledArgument::Variable(name) => ArtifactArgument::Variable { name },
                    },
                )
            })
            .collect(),
        dependencies: &root.dependencies,
        coverage: root.coverage.as_ref().map(|coverage| ArtifactCoverage {
            kind: &coverage.kind,
            offset_argument: coverage.offset_argument.as_deref(),
            limit_argument: coverage.limit_argument.as_deref(),
            default_limit: coverage.default_limit,
            max_limit: coverage.max_limit,
        }),
        selection: artifact_selection(&root.selection),
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
            .map(|(name, argument)| {
                (
                    name.as_str(),
                    match argument {
                        CompiledArgument::Literal { value, .. } => {
                            ArtifactArgument::Literal { value }
                        }
                        CompiledArgument::Variable(name) => ArtifactArgument::Variable { name },
                    },
                )
            })
            .collect(),
        dependencies: &branch.dependencies,
        coverage: branch.coverage.as_ref().map(|coverage| ArtifactCoverage {
            kind: &coverage.kind,
            offset_argument: coverage.offset_argument.as_deref(),
            limit_argument: coverage.limit_argument.as_deref(),
            default_limit: coverage.default_limit,
            max_limit: coverage.max_limit,
        }),
        maintenance: branch.maintenance.as_deref(),
        selection: Box::new(artifact_selection(&branch.selection)),
    }
}

fn render_variables_type(operation: &CompiledOperation, name: &str) -> String {
    if operation.variables.is_empty() {
        return format!("export type {name} = Record<string, never>;");
    }
    let mut lines = vec![format!("export type {name} = {{")];
    for variable in &operation.variables {
        let optional = variable.graphql_type.nullable;
        lines.push(format!(
            "  readonly {}{}: {};",
            quoted_property(&variable.name),
            if optional { "?" } else { "" },
            typescript_type(&variable.graphql_type)
        ));
    }
    lines.push("};".into());
    lines.join("\n")
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
    let commands = serde_json::to_string_pretty(&manifest.commands).map_err(|error| {
        ClientCompileError::manifest(
            "client.render.commands",
            format!("failed to render command artifacts: {error}"),
        )
    })?;
    let projectors = serde_json::to_string_pretty(&manifest.projectors).map_err(|error| {
        ClientCompileError::manifest(
            "client.render.projectors",
            format!("failed to render projector artifacts: {error}"),
        )
    })?;
    Ok(format!(
        "/** GENERATED by dctl client. Manifest declarations are preserved verbatim. */\n\
         export const COMMAND_ARTIFACTS = {commands} as const;\n\
         \n\
         /** Projector topology used by command confirmation/effect runtimes. */\n\
         export const PROJECTOR_ARTIFACTS = {projectors} as const;\n\
         \n\
         export type GeneratedCommandArtifact = (typeof COMMAND_ARTIFACTS)[number];\n"
    ))
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
        distributed_manifest_version: 5,
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
