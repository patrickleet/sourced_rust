use std::collections::{BTreeMap, BTreeSet};

use async_graphql_parser::types::{
    BaseType, Directive, DocumentOperations, Field, OperationDefinition, OperationType, Selection,
    Type,
};
use async_graphql_parser::{parse_query, Pos, Positioned};
use async_graphql_value::Value;
use serde::Serialize;
use serde_json::{Map as JsonMap, Value as JsonValue};

use super::manifest::{
    hash_bytes, ClientManifest, ManifestArgument, ManifestArgumentKind, ManifestField,
    ManifestModel, ManifestRoot, RootKind, RootOperation,
};
use super::{ClientCompileError, ClientDocument, ClientRouteDiscovery, GeneratedRoutePlan};

const MAX_SOURCE_BYTES: usize = 1024 * 1024;
const MAX_VARIABLES: usize = 256;
const MAX_SCALAR_SELECTIONS: usize = 4096;

#[derive(Clone, Debug)]
pub(crate) struct CompiledOperation {
    pub(crate) name: String,
    pub(crate) source_path: String,
    pub(crate) module_path: String,
    pub(crate) export_name: String,
    pub(crate) query_document: String,
    pub(crate) query_hash: String,
    pub(crate) live: Option<CompiledLiveOperation>,
    pub(crate) variables: Vec<CompiledVariable>,
    pub(crate) root: CompiledRoot,
    pub(crate) route: Option<GeneratedRoutePlan>,
}

#[derive(Clone, Debug)]
pub(crate) struct CompiledLiveOperation {
    pub(crate) document: String,
    pub(crate) hash: String,
}

#[derive(Clone, Debug)]
pub(crate) struct CompiledVariable {
    pub(crate) name: String,
    pub(crate) graphql_type: Type,
}

#[derive(Clone, Debug)]
pub(crate) struct CompiledRoot {
    pub(crate) response_key: String,
    pub(crate) field: String,
    pub(crate) cardinality: Cardinality,
    pub(crate) nullable: bool,
    pub(crate) arguments: BTreeMap<String, CompiledArgument>,
    pub(crate) dependencies: Vec<String>,
    pub(crate) coverage: Option<CompiledCoverage>,
    pub(crate) model_id: String,
    pub(crate) identity_fields: Vec<String>,
    pub(crate) fields: Vec<CompiledScalar>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Cardinality {
    One,
    Many,
}

#[derive(Clone, Debug)]
pub(crate) enum CompiledArgument {
    Literal { value: JsonValue, wire: String },
    Variable(String),
}

#[derive(Clone, Debug)]
pub(crate) struct CompiledCoverage {
    pub(crate) kind: String,
    pub(crate) offset_argument: Option<String>,
    pub(crate) limit_argument: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct CompiledScalar {
    pub(crate) response_key: String,
    pub(crate) field: String,
    pub(crate) codec: String,
    pub(crate) nullable: bool,
    pub(crate) expose: bool,
}

pub(crate) fn compile_document(
    document: &ClientDocument,
    manifest: &ClientManifest,
    registrations: &BTreeMap<String, String>,
) -> Result<CompiledOperation, ClientCompileError> {
    if document.source.len() > MAX_SOURCE_BYTES {
        return Err(source_error(
            "client.document.size",
            format!("GraphQL document exceeds the supported {MAX_SOURCE_BYTES}-byte bound"),
            document,
            Pos::default(),
        ));
    }
    let parsed = parse_query(&document.source).map_err(|error| {
        let pos = error.positions().next().unwrap_or_default();
        source_error(
            "client.graphql.parse",
            format!("invalid GraphQL document: {error}"),
            document,
            pos,
        )
    })?;
    if let Some((name, fragment)) = parsed
        .fragments
        .iter()
        .min_by(|left, right| left.0.cmp(right.0))
    {
        return Err(source_error(
            "client.graphql.fragments_unsupported",
            format!(
                "fragment `{name}` is not supported by the first client compiler slice; inline its scalar selections"
            ),
            document,
            fragment.pos,
        ));
    }

    let (name, operation) = match &parsed.operations {
        DocumentOperations::Single(operation) => {
            return Err(source_error(
                "client.operation.named_required",
                "client operations must be explicitly named",
                document,
                operation.pos,
            ));
        }
        DocumentOperations::Multiple(operations) if operations.len() == 1 => {
            let (name, operation) = operations.iter().next().expect("length checked");
            (name.as_str().to_string(), operation)
        }
        DocumentOperations::Multiple(operations) => {
            let position = operations
                .values()
                .map(|operation| operation.pos)
                .min()
                .unwrap_or_default();
            return Err(source_error(
                "client.operation.one_per_document",
                format!(
                    "each client document must contain exactly one named operation; found {}",
                    operations.len()
                ),
                document,
                position,
            ));
        }
    };
    if operation.node.ty != OperationType::Query {
        return Err(source_error(
            "client.operation.query_required",
            "application documents must declare a query; commands are generated from the manifest",
            document,
            operation.pos,
        ));
    }
    let compiler_directives = compiler_directives(&operation.node, document)?;
    let variables = compile_variables(&operation.node, document)?;
    let root_field = single_root_field(&operation.node, document)?;
    reject_directives(&root_field.node.directives, "root field", document)?;

    let root_name = root_field.node.name.node.as_str();
    let root_manifest = manifest
        .root(RootOperation::Query, root_name)
        .ok_or_else(|| {
            source_error(
                "client.root.denied_or_unknown",
                format!("query root `{root_name}` is absent from the selected manifest surface"),
                document,
                root_field.node.name.pos,
            )
        })?;
    if root_manifest.kind == RootKind::Aggregate {
        return Err(source_error(
            "client.root.aggregate_unsupported",
            format!(
                "aggregate root `{root_name}` is not supported by the first client compiler slice"
            ),
            document,
            root_field.node.name.pos,
        ));
    }
    let model = manifest.models.get(&root_manifest.model).ok_or_else(|| {
        ClientCompileError::manifest(
            "client.manifest.root_model",
            format!(
                "root `{root_name}` references absent model `{}`",
                root_manifest.model
            ),
        )
    })?;
    let identity = model.identity().ok_or_else(|| {
        source_error(
            "client.model.embedded_unsupported",
            format!(
                "root `{root_name}` returns embedded model `{}`; the first compiler slice requires a normalized identity",
                model.id
            ),
            document,
            root_field.node.name.pos,
        )
    })?;
    let compiled_arguments =
        compile_arguments(&root_field.node, root_manifest, &variables, document)?;
    let mut fields = compile_scalar_selections(&root_field.node, model, document)?;
    validate_used_variables(&variables, &compiled_arguments, document, operation.pos)?;
    inject_wire_fields(&mut fields, model, document, root_field.pos)?;

    let cardinality = match root_manifest.kind {
        RootKind::List => Cardinality::Many,
        RootKind::ByPk => Cardinality::One,
        RootKind::Aggregate => unreachable!("rejected above"),
    };
    let root = CompiledRoot {
        response_key: root_field.node.response_key().node.to_string(),
        field: root_name.to_string(),
        cardinality,
        nullable: cardinality == Cardinality::One,
        arguments: compiled_arguments,
        dependencies: root_manifest.dependencies.clone(),
        coverage: compile_coverage(root_manifest, document, root_field.pos)?,
        model_id: model.id.clone(),
        identity_fields: identity.iter().map(|field| field.name.clone()).collect(),
        fields,
    };

    let query_document = render_operation(OperationType::Query, &name, &variables, &root)?;
    let query_hash = hash_bytes(query_document.as_bytes());
    let live = if compiler_directives.live {
        Some(compile_live(
            &name,
            &variables,
            &root,
            root_manifest,
            manifest,
            document,
            operation.pos,
        )?)
    } else {
        None
    };
    let route = compile_route(
        &name,
        compiler_directives.load,
        document,
        registrations.get(&name),
        operation.pos,
    )?;
    let module_stem = module_stem(&name);
    Ok(CompiledOperation {
        name: name.clone(),
        source_path: document.path.clone(),
        module_path: format!("operations/{module_stem}.ts"),
        export_name: format!("Operation_{name}"),
        query_document,
        query_hash,
        live,
        variables,
        root,
        route,
    })
}

#[derive(Default)]
struct CompilerDirectives {
    load: bool,
    live: bool,
}

fn compiler_directives(
    operation: &OperationDefinition,
    document: &ClientDocument,
) -> Result<CompilerDirectives, ClientCompileError> {
    let mut result = CompilerDirectives::default();
    let mut seen = BTreeSet::new();
    for directive in &operation.directives {
        let name = directive.node.name.node.as_str();
        if !seen.insert(name) {
            return Err(source_error(
                "client.directive.duplicate",
                format!("directive `@{name}` appears more than once"),
                document,
                directive.pos,
            ));
        }
        if !directive.node.arguments.is_empty() {
            return Err(source_error(
                "client.directive.arguments",
                format!("compiler directive `@{name}` does not accept arguments"),
                document,
                directive.pos,
            ));
        }
        match name {
            "load" => result.load = true,
            "live" => result.live = true,
            "skip" | "include" => {
                return Err(source_error(
                    "client.directive.conditional_unsupported",
                    format!(
                        "conditional directive `@{name}` requires a field-presence plan and is not supported yet"
                    ),
                    document,
                    directive.pos,
                ));
            }
            _ => {
                return Err(source_error(
                    "client.directive.unsupported",
                    format!("operation directive `@{name}` is not supported"),
                    document,
                    directive.pos,
                ));
            }
        }
    }
    Ok(result)
}

fn compile_variables(
    operation: &OperationDefinition,
    document: &ClientDocument,
) -> Result<Vec<CompiledVariable>, ClientCompileError> {
    if operation.variable_definitions.len() > MAX_VARIABLES {
        return Err(source_error(
            "client.variables.bound",
            format!("operation exceeds the supported {MAX_VARIABLES}-variable bound"),
            document,
            operation.selection_set.pos,
        ));
    }
    let mut variables = Vec::with_capacity(operation.variable_definitions.len());
    let mut seen = BTreeSet::new();
    for variable in &operation.variable_definitions {
        let definition = &variable.node;
        let name = definition.name.node.as_str();
        if !seen.insert(name) {
            return Err(source_error(
                "client.variable.duplicate",
                format!("variable `${name}` is defined more than once"),
                document,
                definition.name.pos,
            ));
        }
        reject_directives(&definition.directives, "variable definition", document)?;
        if definition.default_value.is_some() {
            return Err(source_error(
                "client.variable.default_unsupported",
                format!(
                    "variable `${name}` declares a default; pass the effective root argument explicitly so cache identity cannot diverge from server coercion"
                ),
                document,
                definition.name.pos,
            ));
        }
        variables.push(CompiledVariable {
            name: name.to_string(),
            graphql_type: definition.var_type.node.clone(),
        });
    }
    variables.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(variables)
}

fn single_root_field<'a>(
    operation: &'a OperationDefinition,
    document: &ClientDocument,
) -> Result<&'a Positioned<Field>, ClientCompileError> {
    if operation.selection_set.node.items.len() != 1 {
        return Err(source_error(
            "client.operation.single_root",
            format!(
                "causal protocol v2 requires exactly one query root; found {}",
                operation.selection_set.node.items.len()
            ),
            document,
            operation.selection_set.pos,
        ));
    }
    match &operation.selection_set.node.items[0].node {
        Selection::Field(field) => Ok(field),
        Selection::FragmentSpread(fragment) => Err(source_error(
            "client.graphql.fragments_unsupported",
            "a root fragment spread is not supported",
            document,
            fragment.pos,
        )),
        Selection::InlineFragment(fragment) => Err(source_error(
            "client.graphql.fragments_unsupported",
            "a root inline fragment is not supported",
            document,
            fragment.pos,
        )),
    }
}

fn compile_arguments(
    field: &Field,
    root: &ManifestRoot,
    variables: &[CompiledVariable],
    document: &ClientDocument,
) -> Result<BTreeMap<String, CompiledArgument>, ClientCompileError> {
    let manifest_arguments = root
        .arguments
        .iter()
        .map(|argument| (argument.name.as_str(), argument))
        .collect::<BTreeMap<_, _>>();
    let variables = variables
        .iter()
        .map(|variable| (variable.name.as_str(), variable))
        .collect::<BTreeMap<_, _>>();
    let mut result = BTreeMap::new();
    for (name, value) in &field.arguments {
        let name_string = name.node.as_str();
        let Some(manifest_argument) = manifest_arguments.get(name_string) else {
            return Err(source_error(
                "client.argument.denied_or_unknown",
                format!(
                    "argument `{name_string}` is absent from selected root `{}`",
                    root.name
                ),
                document,
                name.pos,
            ));
        };
        if result.contains_key(name_string) {
            return Err(source_error(
                "client.argument.duplicate",
                format!("root argument `{name_string}` appears more than once"),
                document,
                name.pos,
            ));
        }
        let compiled = match &value.node {
            Value::Variable(variable) => {
                let Some(definition) = variables.get(variable.as_str()) else {
                    return Err(source_error(
                        "client.variable.undefined",
                        format!(
                            "argument `{name_string}` references undefined variable `${variable}`"
                        ),
                        document,
                        value.pos,
                    ));
                };
                let expected_type =
                    Type::new(&manifest_argument.graphql_type()).expect("manifest type validated");
                if !variable_type_compatible(&definition.graphql_type, &expected_type) {
                    let actual_type = definition.graphql_type.to_string();
                    let expected_type = expected_type.to_string();
                    return Err(source_error(
                        "client.variable.type_mismatch",
                        format!(
                            "variable `${variable}` used for `{name_string}` has type `{actual_type}`; selected manifest requires `{expected_type}`"
                        ),
                        document,
                        value.pos,
                    ));
                }
                CompiledArgument::Variable(variable.to_string())
            }
            literal => {
                reject_nested_variable(literal, name_string, document, value.pos)?;
                validate_literal(literal, manifest_argument, document, value.pos)?;
                CompiledArgument::Literal {
                    value: value_to_json(literal, document, value.pos)?,
                    wire: render_value(literal, document, value.pos)?,
                }
            }
        };
        result.insert(name_string.to_string(), compiled);
    }
    for argument in &root.arguments {
        if !argument.nullable && !result.contains_key(&argument.name) {
            return Err(source_error(
                "client.argument.required",
                format!(
                    "root `{}` requires argument `{}` of type `{}`",
                    root.name,
                    argument.name,
                    argument.graphql_type()
                ),
                document,
                field.name.pos,
            ));
        }
    }
    Ok(result)
}

fn validate_used_variables(
    variables: &[CompiledVariable],
    arguments: &BTreeMap<String, CompiledArgument>,
    document: &ClientDocument,
    position: Pos,
) -> Result<(), ClientCompileError> {
    let used = arguments
        .values()
        .filter_map(|argument| match argument {
            CompiledArgument::Variable(name) => Some(name.as_str()),
            CompiledArgument::Literal { .. } => None,
        })
        .collect::<BTreeSet<_>>();
    if let Some(variable) = variables
        .iter()
        .find(|variable| !used.contains(variable.name.as_str()))
    {
        return Err(source_error(
            "client.variable.unused",
            format!(
                "variable `${}` is defined but is not a direct root argument",
                variable.name
            ),
            document,
            position,
        ));
    }
    Ok(())
}

fn compile_scalar_selections(
    field: &Field,
    model: &ManifestModel,
    document: &ClientDocument,
) -> Result<Vec<CompiledScalar>, ClientCompileError> {
    if field.selection_set.node.items.is_empty() {
        return Err(source_error(
            "client.selection.empty",
            format!(
                "root `{}` must select at least one scalar field",
                field.name.node
            ),
            document,
            field.selection_set.pos,
        ));
    }
    if field.selection_set.node.items.len() > MAX_SCALAR_SELECTIONS {
        return Err(source_error(
            "client.selection.bound",
            format!("selection exceeds the supported {MAX_SCALAR_SELECTIONS}-field bound"),
            document,
            field.selection_set.pos,
        ));
    }
    let mut result = Vec::with_capacity(field.selection_set.node.items.len());
    let mut response_keys = BTreeSet::new();
    for selection in &field.selection_set.node.items {
        let selected = match &selection.node {
            Selection::Field(field) => field,
            Selection::FragmentSpread(fragment) => {
                return Err(source_error(
                    "client.graphql.fragments_unsupported",
                    format!(
                        "fragment spread `{}` is not supported; inline its scalar selections",
                        fragment.node.fragment_name.node
                    ),
                    document,
                    fragment.pos,
                ));
            }
            Selection::InlineFragment(fragment) => {
                return Err(source_error(
                    "client.graphql.fragments_unsupported",
                    "inline fragments are not supported by the first compiler slice",
                    document,
                    fragment.pos,
                ));
            }
        };
        reject_directives(&selected.node.directives, "selected field", document)?;
        if !selected.node.arguments.is_empty() {
            return Err(source_error(
                "client.selection.field_arguments",
                format!(
                    "scalar field `{}` must not have arguments",
                    selected.node.name.node
                ),
                document,
                selected.pos,
            ));
        }
        let response_key = selected.node.response_key().node.as_str();
        if !response_keys.insert(response_key) {
            return Err(source_error(
                "client.selection.duplicate_response_key",
                format!("response key `{response_key}` appears more than once"),
                document,
                selected.node.response_key().pos,
            ));
        }
        let field_name = selected.node.name.node.as_str();
        if let Some(relationship) = model.relationship(field_name) {
            return Err(source_error(
                "client.selection.relationship_unsupported",
                format!(
                    "relationship `{}` on model `{}` is authorized but not supported by the first compiler slice",
                    relationship.name, model.id
                ),
                document,
                selected.node.name.pos,
            ));
        }
        let manifest_field = if field_name == "__typename" {
            None
        } else {
            Some(model.field(field_name).ok_or_else(|| {
                source_error(
                    "client.selection.denied_or_unknown",
                    format!(
                        "field `{field_name}` is absent from selected model `{}`",
                        model.id
                    ),
                    document,
                    selected.node.name.pos,
                )
            })?)
        };
        if !selected.node.selection_set.node.items.is_empty() {
            return Err(source_error(
                "client.selection.nested_unsupported",
                format!(
                    "field `{field_name}` has a nested selection; only scalar leaves are supported"
                ),
                document,
                selected.node.selection_set.pos,
            ));
        }
        result.push(match manifest_field {
            Some(field) => compiled_scalar(response_key, field, true),
            None => CompiledScalar {
                response_key: response_key.to_string(),
                field: "__typename".into(),
                codec: "string".into(),
                nullable: false,
                expose: true,
            },
        });
    }
    Ok(result)
}

fn compiled_scalar(response_key: &str, field: &ManifestField, expose: bool) -> CompiledScalar {
    CompiledScalar {
        response_key: response_key.to_string(),
        field: field.name.clone(),
        codec: field.codec.clone(),
        nullable: field.nullable,
        expose,
    }
}

fn inject_wire_fields(
    fields: &mut Vec<CompiledScalar>,
    model: &ManifestModel,
    document: &ClientDocument,
    position: Pos,
) -> Result<(), ClientCompileError> {
    let identity = model
        .identity()
        .expect("embedded model rejected before injection");
    let mut response_keys = fields
        .iter()
        .map(|field| field.response_key.clone())
        .collect::<BTreeSet<_>>();
    for identity_field in identity {
        if fields
            .iter()
            .any(|field| field.field == identity_field.name)
        {
            continue;
        }
        let field = model.field(&identity_field.name).ok_or_else(|| {
            source_error(
                "client.selection.identity_denied",
                format!(
                    "normalized identity `{}` is not selectable on model `{}`",
                    identity_field.name, model.id
                ),
                document,
                position,
            )
        })?;
        let response_key = allocate_wire_alias(&identity_field.name, &mut response_keys);
        fields.push(compiled_scalar(&response_key, field, false));
    }
    if !fields.iter().any(|field| field.field == "__typename") {
        let response_key = allocate_wire_alias("typename", &mut response_keys);
        fields.push(CompiledScalar {
            response_key,
            field: "__typename".into(),
            codec: "string".into(),
            nullable: false,
            expose: false,
        });
    }
    Ok(())
}

fn allocate_wire_alias(field: &str, used: &mut BTreeSet<String>) -> String {
    let stem = format!("_distributed_{field}");
    if used.insert(stem.clone()) {
        return stem;
    }
    for suffix in 2_u64.. {
        let candidate = format!("{stem}_{suffix}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!("finite set always admits another suffix")
}

fn compile_coverage(
    root: &ManifestRoot,
    document: &ClientDocument,
    position: Pos,
) -> Result<Option<CompiledCoverage>, ClientCompileError> {
    let Some(pagination) = &root.pagination else {
        return Ok(None);
    };
    if pagination.kind != "offset" {
        return Err(source_error(
            "client.pagination.unsupported",
            format!(
                "root `{}` uses unsupported pagination kind `{}`",
                root.name, pagination.kind
            ),
            document,
            position,
        ));
    }
    Ok(Some(CompiledCoverage {
        kind: "offset".into(),
        offset_argument: root
            .arguments
            .iter()
            .find(|argument| argument.kind == ManifestArgumentKind::Offset)
            .map(|argument| argument.name.clone()),
        limit_argument: root
            .arguments
            .iter()
            .find(|argument| argument.kind == ManifestArgumentKind::Limit)
            .map(|argument| argument.name.clone()),
    }))
}

fn compile_live(
    query_name: &str,
    variables: &[CompiledVariable],
    root: &CompiledRoot,
    query_manifest: &ManifestRoot,
    manifest: &ClientManifest,
    document: &ClientDocument,
    position: Pos,
) -> Result<CompiledLiveOperation, ClientCompileError> {
    if !manifest.capabilities.live_queries || !query_manifest.live {
        return Err(source_error(
            "client.live.unavailable",
            format!(
                "`@live` was requested for `{query_name}`, but selected root `{}` is not live-capable",
                query_manifest.name
            ),
            document,
            position,
        ));
    }
    let subscription = manifest
        .root(RootOperation::Subscription, &query_manifest.name)
        .ok_or_else(|| {
            source_error(
                "client.live.root_missing",
                format!(
                    "`@live` requires subscription root `{}` on the same selected surface",
                    query_manifest.name
                ),
                document,
                position,
            )
        })?;
    if subscription.model != query_manifest.model
        || subscription.kind != query_manifest.kind
        || !arguments_compatible(&subscription.arguments, &query_manifest.arguments)
        || !subscription.live
        || subscription.dependencies != query_manifest.dependencies
        || subscription.pagination != query_manifest.pagination
    {
        return Err(source_error(
            "client.live.root_mismatch",
            format!(
                "subscription root `{}` does not exactly match query model, cardinality, arguments, dependencies, pagination, and live contract",
                subscription.name
            ),
            document,
            position,
        ));
    }
    let live_name = format!("{query_name}_Live");
    let document = render_operation(OperationType::Subscription, &live_name, variables, root)?;
    Ok(CompiledLiveOperation {
        hash: hash_bytes(document.as_bytes()),
        document,
    })
}

fn arguments_compatible(left: &[ManifestArgument], right: &[ManifestArgument]) -> bool {
    let canonical = |arguments: &[ManifestArgument]| {
        arguments
            .iter()
            .map(|argument| {
                (
                    argument.name.clone(),
                    argument.kind,
                    argument.graphql_type(),
                )
            })
            .collect::<BTreeSet<_>>()
    };
    canonical(left) == canonical(right)
}

fn compile_route(
    operation: &str,
    load: bool,
    document: &ClientDocument,
    registration: Option<&String>,
    position: Pos,
) -> Result<Option<GeneratedRoutePlan>, ClientCompileError> {
    if !load {
        return Ok(None);
    }
    if let Some(route) = infer_route(&document.path) {
        if registration.is_some() {
            return Err(source_error(
                "client.route.redundant_registration",
                format!(
                    "`{operation}` is already discovered from `{}`; remove its explicit route registration",
                    document.path
                ),
                document,
                position,
            ));
        }
        return Ok(Some(GeneratedRoutePlan {
            operation: operation.to_string(),
            route,
            source_path: document.path.clone(),
            discovery: ClientRouteDiscovery::Convention,
        }));
    }
    let Some(route) = registration else {
        return Err(source_error(
            "client.route.registration_required",
            format!(
                "`@load` operation `{operation}` is outside `src/routes/**/+page.graphql`; move it there or register `--route {operation}=/route-id`"
            ),
            document,
            position,
        ));
    };
    Ok(Some(GeneratedRoutePlan {
        operation: operation.to_string(),
        route: route.clone(),
        source_path: document.path.clone(),
        discovery: ClientRouteDiscovery::Explicit,
    }))
}

fn infer_route(path: &str) -> Option<String> {
    let marker = "src/routes/";
    let start = if path.starts_with(marker) {
        marker.len()
    } else {
        path.find(&format!("/{marker}"))? + marker.len() + 1
    };
    let rest = path.get(start..)?;
    if rest == "+page.graphql" {
        return Some("/".into());
    }
    let directory = rest.strip_suffix("/+page.graphql")?;
    if directory.is_empty() {
        Some("/".into())
    } else {
        Some(format!("/{directory}"))
    }
}

fn render_operation(
    operation_type: OperationType,
    name: &str,
    variables: &[CompiledVariable],
    root: &CompiledRoot,
) -> Result<String, ClientCompileError> {
    let variable_definitions = if variables.is_empty() {
        String::new()
    } else {
        format!(
            "({})",
            variables
                .iter()
                .map(render_variable)
                .collect::<Result<Vec<_>, _>>()?
                .join(", ")
        )
    };
    let arguments = if root.arguments.is_empty() {
        String::new()
    } else {
        format!(
            "({})",
            root.arguments
                .iter()
                .map(|(name, value)| {
                    Ok(format!(
                        "{name}: {}",
                        match value {
                            CompiledArgument::Literal { wire, .. } => wire.clone(),
                            CompiledArgument::Variable(variable) => format!("${variable}"),
                        }
                    ))
                })
                .collect::<Result<Vec<_>, ClientCompileError>>()?
                .join(", ")
        )
    };
    let root_prefix = if root.response_key == root.field {
        root.field.clone()
    } else {
        format!("{}: {}", root.response_key, root.field)
    };
    let mut lines = vec![format!("{operation_type} {name}{variable_definitions} {{")];
    lines.push(format!("  {root_prefix}{arguments} {{"));
    for field in &root.fields {
        if field.response_key == field.field {
            lines.push(format!("    {}", field.field));
        } else {
            lines.push(format!("    {}: {}", field.response_key, field.field));
        }
    }
    lines.push("  }".into());
    lines.push("}".into());
    Ok(format!("{}\n", lines.join("\n")))
}

fn render_variable(variable: &CompiledVariable) -> Result<String, ClientCompileError> {
    Ok(format!("${}: {}", variable.name, variable.graphql_type))
}

fn render_value(
    value: &Value,
    document: &ClientDocument,
    position: Pos,
) -> Result<String, ClientCompileError> {
    match value {
        Value::Variable(variable) => Ok(format!("${variable}")),
        Value::Null => Ok("null".into()),
        Value::Boolean(value) => Ok(value.to_string()),
        Value::Number(value) => Ok(value.to_string()),
        Value::String(value) => Ok(Value::String(value.clone()).to_string()),
        Value::Binary(_) => Err(source_error(
            "client.literal.binary",
            "binary GraphQL literals are not portable to the JavaScript replica",
            document,
            position,
        )),
        Value::Enum(value) => Ok(value.to_string()),
        Value::List(values) => Ok(format!(
            "[{}]",
            values
                .iter()
                .map(|value| render_value(value, document, position))
                .collect::<Result<Vec<_>, _>>()?
                .join(", ")
        )),
        Value::Object(values) => {
            let sorted = values.iter().collect::<BTreeMap<_, _>>();
            Ok(format!(
                "{{{}}}",
                sorted
                    .into_iter()
                    .map(|(name, value)| {
                        Ok(format!(
                            "{name}: {}",
                            render_value(value, document, position)?
                        ))
                    })
                    .collect::<Result<Vec<_>, ClientCompileError>>()?
                    .join(", ")
            ))
        }
    }
}

fn variable_type_compatible(variable: &Type, argument: &Type) -> bool {
    if !argument.nullable && variable.nullable {
        return false;
    }
    match (&variable.base, &argument.base) {
        (BaseType::Named(variable), BaseType::Named(argument)) => variable == argument,
        (BaseType::List(variable), BaseType::List(argument)) => {
            variable_type_compatible(variable, argument)
        }
        _ => false,
    }
}

fn reject_directives(
    directives: &[Positioned<Directive>],
    owner: &str,
    document: &ClientDocument,
) -> Result<(), ClientCompileError> {
    let Some(directive) = directives.first() else {
        return Ok(());
    };
    let name = directive.node.name.node.as_str();
    let (code, message) = if matches!(name, "skip" | "include") {
        (
            "client.directive.conditional_unsupported",
            format!(
                "conditional directive `@{name}` on {owner} requires a field-presence plan and is not supported yet"
            ),
        )
    } else {
        (
            "client.directive.unsupported",
            format!("directive `@{name}` on {owner} is not supported"),
        )
    };
    Err(source_error(code, message, document, directive.pos))
}

fn reject_nested_variable(
    value: &Value,
    argument: &str,
    document: &ClientDocument,
    position: Pos,
) -> Result<(), ClientCompileError> {
    let nested = match value {
        Value::Variable(_) => false,
        Value::List(values) => values.iter().any(contains_variable),
        Value::Object(values) => values.values().any(contains_variable),
        _ => false,
    };
    if nested {
        return Err(source_error(
            "client.argument.nested_variable",
            format!(
                "argument `{argument}` mixes variables inside a literal; pass the complete root argument as one variable"
            ),
            document,
            position,
        ));
    }
    Ok(())
}

fn contains_variable(value: &Value) -> bool {
    match value {
        Value::Variable(_) => true,
        Value::List(values) => values.iter().any(contains_variable),
        Value::Object(values) => values.values().any(contains_variable),
        _ => false,
    }
}

fn validate_literal(
    value: &Value,
    argument: &ManifestArgument,
    document: &ClientDocument,
    position: Pos,
) -> Result<(), ClientCompileError> {
    if matches!(value, Value::Null) {
        if argument.nullable {
            return Ok(());
        }
        return Err(source_error(
            "client.argument.null",
            format!(
                "argument `{}` is non-null in the selected manifest",
                argument.name
            ),
            document,
            position,
        ));
    }
    if argument.list && !matches!(value, Value::List(_)) {
        return Err(source_error(
            "client.argument.list_literal",
            format!(
                "argument `{}` requires an explicit list literal or a variable of type `{}`",
                argument.name,
                argument.graphql_type()
            ),
            document,
            position,
        ));
    }
    if argument.list {
        return Ok(());
    }
    let valid = match argument.type_name.as_str() {
        "Boolean" => matches!(value, Value::Boolean(_)),
        "Int" => matches!(value, Value::Number(number) if number.is_i64() || number.is_u64()),
        "Float" | "BigInt" => matches!(value, Value::Number(_)),
        "ID" | "String" | "Bytea" | "Timestamptz" => matches!(value, Value::String(_)),
        "JSON" => !matches!(value, Value::Binary(_) | Value::Variable(_)),
        _ => matches!(value, Value::Object(_) | Value::Enum(_) | Value::List(_)),
    };
    if valid {
        Ok(())
    } else {
        Err(source_error(
            "client.argument.literal_type",
            format!(
                "literal for argument `{}` does not match manifest type `{}`",
                argument.name,
                argument.graphql_type()
            ),
            document,
            position,
        ))
    }
}

fn value_to_json(
    value: &Value,
    document: &ClientDocument,
    position: Pos,
) -> Result<JsonValue, ClientCompileError> {
    match value {
        Value::Variable(variable) => Err(source_error(
            "client.argument.nested_variable",
            format!("nested variable `${variable}` is not a literal"),
            document,
            position,
        )),
        Value::Null => Ok(JsonValue::Null),
        Value::Number(value) => Ok(JsonValue::Number(value.clone())),
        Value::String(value) => Ok(JsonValue::String(value.clone())),
        Value::Boolean(value) => Ok(JsonValue::Bool(*value)),
        Value::Binary(_) => Err(source_error(
            "client.literal.binary",
            "binary GraphQL literals are not portable to the JavaScript replica",
            document,
            position,
        )),
        Value::Enum(value) => Ok(JsonValue::String(value.to_string())),
        Value::List(values) => Ok(JsonValue::Array(
            values
                .iter()
                .map(|value| value_to_json(value, document, position))
                .collect::<Result<_, _>>()?,
        )),
        Value::Object(values) => {
            let mut object = JsonMap::new();
            let mut values = values.iter().collect::<Vec<_>>();
            values.sort_by(|left, right| left.0.cmp(right.0));
            for (name, value) in values {
                object.insert(name.to_string(), value_to_json(value, document, position)?);
            }
            Ok(JsonValue::Object(object))
        }
    }
}

pub(crate) fn typescript_type(graphql_type: &Type) -> String {
    let base = match &graphql_type.base {
        BaseType::Named(name) => typescript_named_type(name.as_str()),
        BaseType::List(item) => {
            format!("readonly {}[]", parenthesize_union(&typescript_type(item)))
        }
    };
    if graphql_type.nullable {
        format!("{base} | null")
    } else {
        base
    }
}

pub(crate) fn typescript_scalar(
    field: &CompiledScalar,
) -> Result<&'static str, ClientCompileError> {
    match field.codec.as_str() {
        "boolean" => Ok("boolean"),
        "float64" | "int32" | "json_number_precision_limited" => Ok("number"),
        "string" | "base64" | "string_unvalidated_timestamp" => Ok("string"),
        "json" => Ok("unknown"),
        codec => Err(ClientCompileError::manifest(
            "client.scalar.codec_unsupported",
            format!(
                "field `{}` uses unsupported TypeScript codec `{codec}`",
                field.field
            ),
        )),
    }
}

fn typescript_named_type(name: &str) -> String {
    match name {
        "Boolean" => "boolean".into(),
        "Float" | "Int" | "BigInt" => "number".into(),
        "ID" | "String" | "Bytea" | "Timestamptz" => "string".into(),
        "JSON" => "unknown".into(),
        _ => "Readonly<Record<string, unknown>>".into(),
    }
}

fn parenthesize_union(value: &str) -> String {
    if value.contains(" | ") {
        format!("({value})")
    } else {
        value.to_string()
    }
}

fn module_stem(name: &str) -> String {
    let mut result = String::new();
    for (index, character) in name.chars().enumerate() {
        if character == '_' {
            if !result.ends_with('-') {
                result.push('-');
            }
        } else if character.is_ascii_uppercase() {
            if index > 0 && !result.ends_with('-') {
                result.push('-');
            }
            result.push(character.to_ascii_lowercase());
        } else {
            result.push(character.to_ascii_lowercase());
        }
    }
    result
}

fn source_error(
    code: &'static str,
    message: impl Into<String>,
    document: &ClientDocument,
    position: Pos,
) -> ClientCompileError {
    ClientCompileError::source(
        code,
        message,
        &document.path,
        position.line.max(1),
        position.column.max(1),
    )
}

#[cfg(test)]
mod local_tests {
    use super::{infer_route, module_stem};

    #[test]
    fn route_convention_is_narrow() {
        assert_eq!(
            infer_route("src/routes/todos/+page.graphql").as_deref(),
            Some("/todos")
        );
        assert_eq!(
            infer_route("/tmp/app/src/routes/+page.graphql").as_deref(),
            Some("/")
        );
        assert_eq!(infer_route("src/lib/todos.graphql"), None);
        assert_eq!(infer_route("src/routes/todos/query.graphql"), None);
    }

    #[test]
    fn module_names_are_portable() {
        assert_eq!(module_stem("TodosForUser"), "todos-for-user");
        assert_eq!(module_stem("todos_for_user"), "todos-for-user");
    }
}
