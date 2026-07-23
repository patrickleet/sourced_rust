use std::collections::{BTreeMap, BTreeSet, HashMap};

use async_graphql_parser::types::{
    BaseType, Directive, DocumentOperations, Field, FragmentDefinition, OperationDefinition,
    OperationType, Selection, SelectionSet, Type,
};
use async_graphql_parser::{parse_query, Pos, Positioned};
use async_graphql_value::{Name, Value};
use serde::Serialize;
use serde_json::{Map as JsonMap, Value as JsonValue};

use super::manifest::{
    hash_bytes, ClientManifest, ManifestAggregateSemantics, ManifestArgument, ManifestArgumentKind,
    ManifestExecutionLimits, ManifestField, ManifestModel, ManifestPagination,
    ManifestRelationshipKind, ManifestRelationshipMaintenance, ManifestRoot, RootKind,
    RootOperation,
};
use super::{ClientCompileError, ClientDocument, ClientRouteDiscovery, GeneratedRoutePlan};

const MAX_SOURCE_BYTES: usize = 1024 * 1024;
const MAX_VARIABLES: usize = 256;
const MAX_OBJECT_DEPTH: usize = 64;
const MAX_EXPANDED_SELECTIONS: usize = 10_000;

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
    pub(crate) selection: CompiledObject,
}

#[derive(Clone, Debug)]
pub(crate) struct CompiledObject {
    pub(crate) typename: String,
    pub(crate) storage: CompiledStorage,
    pub(crate) members: Vec<CompiledMember>,
}

#[derive(Clone, Debug)]
pub(crate) enum CompiledMember {
    Scalar(CompiledScalar),
    Branch(Box<CompiledBranch>),
}

#[derive(Clone, Debug)]
pub(crate) enum CompiledStorage {
    Normalized {
        model_id: String,
        identity_fields: Vec<String>,
    },
    Embedded,
}

#[derive(Clone, Debug)]
pub(crate) struct CompiledBranch {
    pub(crate) semantic: CompiledBranchSemantic,
    pub(crate) response_key: String,
    pub(crate) field: String,
    pub(crate) cardinality: Cardinality,
    pub(crate) nullable: bool,
    pub(crate) arguments: BTreeMap<String, CompiledArgument>,
    pub(crate) dependencies: Vec<String>,
    pub(crate) coverage: Option<CompiledCoverage>,
    pub(crate) maintenance: Option<String>,
    pub(crate) selection: CompiledObject,
    relationship_kind: Option<ManifestRelationshipKind>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CompiledBranchSemantic {
    Relationship,
    Aggregate,
    AggregateFields,
    AggregateNodes,
}

struct MergedField<'a> {
    first: &'a Positioned<Field>,
    selection_sets: Vec<&'a Positioned<SelectionSet>>,
    canonical_arguments: Vec<(String, String)>,
}

struct FragmentExpander<'ast, 'source> {
    fragments: &'ast HashMap<Name, Positioned<FragmentDefinition>>,
    document: &'source ClientDocument,
    state: ExpansionState,
}

#[derive(Default)]
struct ExpansionState {
    used_fragments: BTreeSet<String>,
    active_fragments: Vec<String>,
    expanded_units: usize,
}

#[derive(Default)]
struct FragmentGraphState {
    active_fragments: Vec<String>,
    completed_fragments: BTreeSet<String>,
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
    pub(crate) default_limit: Option<u64>,
    pub(crate) max_limit: Option<u64>,
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
    let mut expander = FragmentExpander::new(&parsed.fragments, document);
    let root_fields =
        expander.merge_object(&[&operation.node.selection_set], "Query", 1, "root field")?;
    let root_field = single_root_field(root_fields, &operation.node, document)?;

    let root_name = root_field.first.node.name.node.as_str();
    let root_manifest = manifest
        .root(RootOperation::Query, root_name)
        .ok_or_else(|| {
            source_error(
                "client.root.denied_or_unknown",
                format!("query root `{root_name}` is absent from the selected manifest surface"),
                document,
                root_field.first.node.name.pos,
            )
        })?;
    let model = manifest.models.get(&root_manifest.model).ok_or_else(|| {
        ClientCompileError::manifest(
            "client.manifest.root_model",
            format!(
                "root `{root_name}` references absent model `{}`",
                root_manifest.model
            ),
        )
    })?;
    validate_reachable_fragment_graph(&parsed.fragments, document, &operation.node.selection_set)?;
    let mut used_variables = BTreeSet::new();
    let compiled_arguments = compile_arguments(
        &root_field.first.node,
        &root_manifest.name,
        &root_manifest.arguments,
        &variables,
        &mut used_variables,
        document,
    )?;
    let selection = match root_manifest.kind {
        RootKind::List | RootKind::ByPk => compile_model_object(
            &root_field,
            model,
            manifest,
            &variables,
            &mut used_variables,
            document,
            &mut expander,
            2,
        )?,
        RootKind::Aggregate => {
            let semantics = root_manifest.aggregate.as_ref().ok_or_else(|| {
                ClientCompileError::manifest(
                    "client.manifest.aggregate_semantics",
                    format!("aggregate root `{root_name}` has no aggregate semantics"),
                )
            })?;
            compile_aggregate_object(
                &root_field,
                model,
                semantics,
                &root_manifest.arguments,
                &root_manifest.dependencies,
                manifest,
                &variables,
                &mut used_variables,
                document,
                &mut expander,
                2,
            )?
        }
    };
    validate_used_variables(&variables, &used_variables, document, operation.pos)?;
    expander.reject_unused_fragments()?;

    let cardinality = match root_manifest.kind {
        RootKind::List => Cardinality::Many,
        RootKind::ByPk => Cardinality::One,
        RootKind::Aggregate => Cardinality::One,
    };
    let root = CompiledRoot {
        response_key: root_field.first.node.response_key().node.to_string(),
        field: root_name.to_string(),
        cardinality,
        nullable: matches!(root_manifest.kind, RootKind::ByPk | RootKind::Aggregate),
        arguments: compiled_arguments,
        dependencies: root_manifest.dependencies.clone(),
        coverage: match root_manifest.kind {
            RootKind::Aggregate => Some(complete_coverage()),
            RootKind::List | RootKind::ByPk => compile_coverage(
                root_manifest.pagination.as_ref(),
                &root_manifest.arguments,
                &root_manifest.name,
                document,
                root_field.first.pos,
            )?,
        },
        selection,
    };
    validate_execution_limits(
        &root,
        root_manifest.kind,
        &manifest.execution,
        document,
        operation.pos,
    )?;

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

fn validate_execution_limits(
    root: &CompiledRoot,
    kind: RootKind,
    execution: &ManifestExecutionLimits,
    document: &ClientDocument,
    position: Pos,
) -> Result<(), ClientCompileError> {
    let depth = compiled_object_depth(&root.selection, 1);
    if depth > execution.max_depth {
        return Err(source_error(
            "client.operation.depth_limit",
            format!(
                "compiled operation depth {depth} exceeds the selected service max_depth {}",
                execution.max_depth
            ),
            document,
            position,
        ));
    }

    let weights = &execution.complexity;
    let child = compiled_object_complexity(&root.selection, weights);
    let complexity = match kind {
        RootKind::List => weights
            .list_root
            .saturating_add(weights.list_fanout.saturating_mul(child)),
        RootKind::ByPk => weights.by_pk.saturating_add(child),
        RootKind::Aggregate => weights.aggregate.saturating_add(child),
    };
    if complexity > execution.max_complexity {
        return Err(source_error(
            "client.operation.complexity_limit",
            format!(
                "compiled operation complexity {complexity} exceeds the selected service max_complexity {}",
                execution.max_complexity
            ),
            document,
            position,
        ));
    }
    Ok(())
}

fn compiled_object_depth(selection: &CompiledObject, parent_depth: u64) -> u64 {
    selection
        .members
        .iter()
        .map(|member| {
            let field_depth = parent_depth.saturating_add(1);
            match member {
                CompiledMember::Scalar(_) => field_depth,
                CompiledMember::Branch(branch) => {
                    field_depth.max(compiled_object_depth(&branch.selection, field_depth))
                }
            }
        })
        .max()
        .unwrap_or(parent_depth)
}

fn compiled_object_complexity(
    selection: &CompiledObject,
    weights: &super::manifest::ManifestComplexityWeights,
) -> u64 {
    selection.members.iter().fold(0, |total, member| {
        let cost = match member {
            CompiledMember::Scalar(_) => weights.scalar,
            CompiledMember::Branch(branch) => match branch.semantic {
                CompiledBranchSemantic::Relationship => {
                    let child = compiled_object_complexity(&branch.selection, weights);
                    match branch
                        .relationship_kind
                        .expect("relationship branches retain their manifest kind")
                    {
                        ManifestRelationshipKind::BelongsTo => {
                            weights.belongs_to.saturating_add(child)
                        }
                        ManifestRelationshipKind::HasMany => weights
                            .has_many
                            .saturating_add(weights.list_fanout.saturating_mul(child)),
                        ManifestRelationshipKind::ManyToMany => weights
                            .m2m
                            .saturating_add(weights.list_fanout.saturating_mul(child)),
                    }
                }
                CompiledBranchSemantic::Aggregate => weights
                    .aggregate
                    .saturating_add(compiled_object_complexity(&branch.selection, weights)),
                CompiledBranchSemantic::AggregateFields => weights.scalar,
                CompiledBranchSemantic::AggregateNodes => weights
                    .list_fanout
                    .saturating_mul(compiled_object_complexity(&branch.selection, weights))
                    .max(weights.scalar),
            },
        };
        total.saturating_add(cost)
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

fn validate_reachable_fragment_graph<'ast>(
    fragments: &'ast HashMap<Name, Positioned<FragmentDefinition>>,
    document: &ClientDocument,
    selection_set: &'ast Positioned<SelectionSet>,
) -> Result<(), ClientCompileError> {
    validate_fragment_selection_set(
        fragments,
        document,
        &mut FragmentGraphState::default(),
        selection_set,
        1,
    )
}

fn validate_fragment_selection_set<'ast>(
    fragments: &'ast HashMap<Name, Positioned<FragmentDefinition>>,
    document: &ClientDocument,
    state: &mut FragmentGraphState,
    selection_set: &'ast Positioned<SelectionSet>,
    depth: usize,
) -> Result<(), ClientCompileError> {
    check_expansion_depth(depth, document, selection_set.pos)?;
    for selection in &selection_set.node.items {
        match &selection.node {
            Selection::Field(field) => {
                if !field.node.selection_set.node.items.is_empty() {
                    validate_fragment_selection_set(
                        fragments,
                        document,
                        state,
                        &field.node.selection_set,
                        depth + 1,
                    )?;
                }
            }
            Selection::InlineFragment(fragment) => {
                validate_fragment_selection_set(
                    fragments,
                    document,
                    state,
                    &fragment.node.selection_set,
                    depth + 1,
                )?;
            }
            Selection::FragmentSpread(spread) => {
                let name = spread.node.fragment_name.node.as_str();
                let Some(definition) = fragments.get(name) else {
                    return Err(source_error(
                        "client.graphql.fragment_undefined",
                        format!("fragment spread `{name}` has no definition in this document"),
                        document,
                        spread.node.fragment_name.pos,
                    ));
                };
                if let Some(cycle_start) = state
                    .active_fragments
                    .iter()
                    .position(|active| active == name)
                {
                    let mut cycle = state.active_fragments[cycle_start..].to_vec();
                    cycle.push(name.to_string());
                    return Err(source_error(
                        "client.graphql.fragment_cycle",
                        format!("fragment expansion cycle: {}", cycle.join(" -> ")),
                        document,
                        spread.node.fragment_name.pos,
                    ));
                }
                if state.completed_fragments.contains(name) {
                    continue;
                }
                check_expansion_depth(depth + 1, document, spread.pos)?;
                state.active_fragments.push(name.to_string());
                let result = validate_fragment_selection_set(
                    fragments,
                    document,
                    state,
                    &definition.node.selection_set,
                    depth + 1,
                );
                state.active_fragments.pop();
                result?;
                state.completed_fragments.insert(name.to_string());
            }
        }
    }
    Ok(())
}

impl<'ast, 'source> FragmentExpander<'ast, 'source> {
    fn new(
        fragments: &'ast HashMap<Name, Positioned<FragmentDefinition>>,
        document: &'source ClientDocument,
    ) -> Self {
        Self {
            fragments,
            document,
            state: ExpansionState::default(),
        }
    }

    fn merge_object(
        &mut self,
        selection_sets: &[&'ast Positioned<SelectionSet>],
        typename: &str,
        depth: usize,
        field_owner: &str,
    ) -> Result<Vec<MergedField<'ast>>, ClientCompileError> {
        let position = selection_sets
            .first()
            .map_or_else(Pos::default, |selection_set| selection_set.pos);
        check_expansion_depth(depth, self.document, position)?;
        count_expansion_unit(&mut self.state, self.document, position)?;

        let mut fields = Vec::new();
        let mut response_keys = BTreeMap::new();
        for selection_set in selection_sets {
            expand_selection_set(
                self.fragments,
                self.document,
                &mut self.state,
                selection_set,
                typename,
                depth,
                field_owner,
                &mut fields,
                &mut response_keys,
            )?;
        }
        Ok(fields)
    }

    fn reject_unused_fragments(&self) -> Result<(), ClientCompileError> {
        let unused = self
            .fragments
            .iter()
            .filter(|(name, _)| !self.state.used_fragments.contains(name.as_str()))
            .min_by(|left, right| {
                (left.1.pos, left.0.as_str()).cmp(&(right.1.pos, right.0.as_str()))
            });
        let Some((name, definition)) = unused else {
            return Ok(());
        };
        Err(source_error(
            "client.graphql.fragment_unused",
            format!("fragment `{name}` is not reachable from the document operation"),
            self.document,
            definition.pos,
        ))
    }
}

#[allow(clippy::too_many_arguments)]
fn expand_selection_set<'ast>(
    fragments: &'ast HashMap<Name, Positioned<FragmentDefinition>>,
    document: &ClientDocument,
    state: &mut ExpansionState,
    selection_set: &'ast Positioned<SelectionSet>,
    typename: &str,
    depth: usize,
    field_owner: &str,
    fields: &mut Vec<MergedField<'ast>>,
    response_keys: &mut BTreeMap<String, usize>,
) -> Result<(), ClientCompileError> {
    check_expansion_depth(depth, document, selection_set.pos)?;
    for selection in &selection_set.node.items {
        count_expansion_unit(state, document, selection.pos)?;
        match &selection.node {
            Selection::Field(field) => {
                merge_field(document, field, field_owner, fields, response_keys)?
            }
            Selection::FragmentSpread(spread) => {
                reject_directives(
                    &spread.node.directives,
                    &format!(
                        "fragment spread `{}`",
                        spread.node.fragment_name.node.as_str()
                    ),
                    document,
                )?;
                let name = spread.node.fragment_name.node.as_str();
                let Some(definition) = fragments.get(name) else {
                    return Err(source_error(
                        "client.graphql.fragment_undefined",
                        format!("fragment spread `{name}` has no definition in this document"),
                        document,
                        spread.node.fragment_name.pos,
                    ));
                };
                state.used_fragments.insert(name.to_string());
                reject_directives(
                    &definition.node.directives,
                    &format!("fragment definition `{name}`"),
                    document,
                )?;
                require_fragment_type(
                    definition.node.type_condition.node.on.node.as_str(),
                    typename,
                    name,
                    document,
                    definition.node.type_condition.pos,
                )?;
                if let Some(cycle_start) = state
                    .active_fragments
                    .iter()
                    .position(|active| active == name)
                {
                    let mut cycle = state.active_fragments[cycle_start..].to_vec();
                    cycle.push(name.to_string());
                    return Err(source_error(
                        "client.graphql.fragment_cycle",
                        format!("fragment expansion cycle: {}", cycle.join(" -> ")),
                        document,
                        spread.node.fragment_name.pos,
                    ));
                }
                check_expansion_depth(depth + 1, document, spread.pos)?;
                state.active_fragments.push(name.to_string());
                let result = expand_selection_set(
                    fragments,
                    document,
                    state,
                    &definition.node.selection_set,
                    typename,
                    depth + 1,
                    field_owner,
                    fields,
                    response_keys,
                );
                state.active_fragments.pop();
                result?;
            }
            Selection::InlineFragment(fragment) => {
                reject_directives(&fragment.node.directives, "inline fragment", document)?;
                if let Some(condition) = &fragment.node.type_condition {
                    require_fragment_type(
                        condition.node.on.node.as_str(),
                        typename,
                        "inline fragment",
                        document,
                        condition.pos,
                    )?;
                }
                check_expansion_depth(depth + 1, document, fragment.pos)?;
                expand_selection_set(
                    fragments,
                    document,
                    state,
                    &fragment.node.selection_set,
                    typename,
                    depth + 1,
                    field_owner,
                    fields,
                    response_keys,
                )?;
            }
        }
    }
    Ok(())
}

fn merge_field<'ast>(
    document: &ClientDocument,
    field: &'ast Positioned<Field>,
    field_owner: &str,
    fields: &mut Vec<MergedField<'ast>>,
    response_keys: &mut BTreeMap<String, usize>,
) -> Result<(), ClientCompileError> {
    reject_directives(&field.node.directives, field_owner, document)?;
    let response_key = field.node.response_key().node.as_str();
    let canonical_arguments = canonical_field_arguments(&field.node, document)?;
    let is_object = !field.node.selection_set.node.items.is_empty();

    if let Some(index) = response_keys.get(response_key).copied() {
        let first = &mut fields[index];
        let first_is_object = !first.selection_sets.is_empty();
        if first.first.node.name.node != field.node.name.node
            || first.canonical_arguments != canonical_arguments
            || first_is_object != is_object
        {
            let first_position = first.first.node.response_key().pos;
            return Err(source_error(
                "client.selection.conflict",
                format!(
                    "response key `{response_key}` conflicts with its first selection at {}:{}",
                    first_position.line.max(1),
                    first_position.column.max(1)
                ),
                document,
                field.node.response_key().pos,
            ));
        }
        if is_object {
            first.selection_sets.push(&field.node.selection_set);
        }
        return Ok(());
    }

    response_keys.insert(response_key.to_string(), fields.len());
    fields.push(MergedField {
        first: field,
        selection_sets: is_object
            .then_some(&field.node.selection_set)
            .into_iter()
            .collect(),
        canonical_arguments,
    });
    Ok(())
}

fn canonical_field_arguments(
    field: &Field,
    document: &ClientDocument,
) -> Result<Vec<(String, String)>, ClientCompileError> {
    let mut arguments = field
        .arguments
        .iter()
        .map(|(name, value)| {
            Ok((
                name.node.to_string(),
                render_value(&value.node, document, value.pos)?,
            ))
        })
        .collect::<Result<Vec<_>, ClientCompileError>>()?;
    arguments.sort();
    Ok(arguments)
}

fn require_fragment_type(
    actual: &str,
    expected: &str,
    fragment: &str,
    document: &ClientDocument,
    position: Pos,
) -> Result<(), ClientCompileError> {
    if actual == expected {
        return Ok(());
    }
    Err(source_error(
        "client.graphql.fragment_type",
        format!(
            "{fragment} has type condition `{actual}` but the current concrete type is `{expected}`"
        ),
        document,
        position,
    ))
}

fn check_expansion_depth(
    depth: usize,
    document: &ClientDocument,
    position: Pos,
) -> Result<(), ClientCompileError> {
    if depth <= MAX_OBJECT_DEPTH {
        return Ok(());
    }
    Err(source_error(
        "client.selection.depth",
        format!("selection expansion exceeds the supported {MAX_OBJECT_DEPTH}-level depth"),
        document,
        position,
    ))
}

fn count_expansion_unit(
    state: &mut ExpansionState,
    document: &ClientDocument,
    position: Pos,
) -> Result<(), ClientCompileError> {
    if state.expanded_units >= MAX_EXPANDED_SELECTIONS {
        return Err(source_error(
            "client.selection.expansion_bound",
            format!(
                "expanded selection exceeds the supported {MAX_EXPANDED_SELECTIONS}-unit bound"
            ),
            document,
            position,
        ));
    }
    state.expanded_units += 1;
    Ok(())
}

fn single_root_field<'a>(
    mut fields: Vec<MergedField<'a>>,
    operation: &OperationDefinition,
    document: &ClientDocument,
) -> Result<MergedField<'a>, ClientCompileError> {
    if fields.len() != 1 {
        return Err(source_error(
            "client.operation.single_root",
            format!(
                "causal protocol v2 requires exactly one query root; found {}",
                fields.len()
            ),
            document,
            operation.selection_set.pos,
        ));
    }
    Ok(fields.pop().expect("length checked"))
}

fn compile_arguments(
    field: &Field,
    owner: &str,
    allowed_arguments: &[ManifestArgument],
    variables: &[CompiledVariable],
    used_variables: &mut BTreeSet<String>,
    document: &ClientDocument,
) -> Result<BTreeMap<String, CompiledArgument>, ClientCompileError> {
    let manifest_arguments = allowed_arguments
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
                format!("argument `{name_string}` is absent from selected field `{owner}`",),
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
                used_variables.insert(variable.to_string());
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
    for argument in allowed_arguments {
        if !argument.nullable && !result.contains_key(&argument.name) {
            return Err(source_error(
                "client.argument.required",
                format!(
                    "field `{owner}` requires argument `{}` of type `{}`",
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
    used: &BTreeSet<String>,
    document: &ClientDocument,
    position: Pos,
) -> Result<(), ClientCompileError> {
    if let Some(variable) = variables
        .iter()
        .find(|variable| !used.contains(&variable.name))
    {
        return Err(source_error(
            "client.variable.unused",
            format!(
                "variable `${}` is defined but is not used by the compiled operation",
                variable.name,
            ),
            document,
            position,
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn compile_model_object<'ast>(
    field: &MergedField<'ast>,
    model: &ManifestModel,
    manifest: &ClientManifest,
    variables: &[CompiledVariable],
    used_variables: &mut BTreeSet<String>,
    document: &ClientDocument,
    expander: &mut FragmentExpander<'ast, '_>,
    depth: usize,
) -> Result<CompiledObject, ClientCompileError> {
    let selected_fields = expander.merge_object(
        &field.selection_sets,
        &model.typename,
        depth,
        "selected field",
    )?;
    if selected_fields.is_empty() {
        return Err(source_error(
            "client.selection.empty",
            format!(
                "object field `{}` must select at least one field",
                field.first.node.name.node
            ),
            document,
            field.first.node.selection_set.pos,
        ));
    }
    let mut result = Vec::with_capacity(selected_fields.len());
    for selected in selected_fields {
        let response_key = selected.first.node.response_key().node.as_str();
        let field_name = selected.first.node.name.node.as_str();
        if let Some(relationship) = model.relationship(field_name) {
            if selected.selection_sets.is_empty() {
                return Err(source_error(
                    "client.selection.object_required",
                    format!(
                        "relationship `{}` on model `{}` requires an object selection",
                        relationship.name, model.id
                    ),
                    document,
                    selected.first.node.selection_set.pos,
                ));
            }
            let target = manifest
                .models
                .get(&relationship.target_model)
                .ok_or_else(|| {
                    ClientCompileError::manifest(
                        "client.manifest.relationship_target",
                        format!(
                            "relationship `{}.{}` references absent target model `{}`",
                            model.id, relationship.name, relationship.target_model
                        ),
                    )
                })?;
            let arguments = compile_arguments(
                &selected.first.node,
                &format!("{}.{}", model.id, relationship.name),
                &relationship.arguments,
                variables,
                used_variables,
                document,
            )?;
            let selection = compile_model_object(
                &selected,
                target,
                manifest,
                variables,
                used_variables,
                document,
                expander,
                depth + 1,
            )?;
            let cardinality = if relationship.list {
                Cardinality::Many
            } else {
                Cardinality::One
            };
            let maintenance = match relationship.maintenance {
                ManifestRelationshipMaintenance::Local => "local",
                ManifestRelationshipMaintenance::Revalidate => "revalidate",
            };
            result.push(CompiledMember::Branch(Box::new(CompiledBranch {
                semantic: CompiledBranchSemantic::Relationship,
                response_key: response_key.to_string(),
                field: relationship.name.clone(),
                cardinality,
                nullable: relationship.nullable,
                arguments,
                dependencies: relationship.dependencies.clone(),
                coverage: compile_coverage(
                    relationship.pagination.as_ref(),
                    &relationship.arguments,
                    &format!("{}.{}", model.id, relationship.name),
                    document,
                    selected.first.pos,
                )?,
                maintenance: Some(maintenance.into()),
                selection,
                relationship_kind: Some(relationship.kind),
            })));
            continue;
        }
        if let Some((relationship, aggregate)) = model.relationships.iter().find_map(|candidate| {
            candidate
                .aggregate
                .as_ref()
                .filter(|aggregate| aggregate.name == field_name)
                .map(|aggregate| (candidate, aggregate))
        }) {
            if selected.selection_sets.is_empty() {
                return Err(source_error(
                    "client.selection.object_required",
                    format!(
                        "relationship aggregate `{}` on model `{}` requires an object selection",
                        aggregate.name, model.id
                    ),
                    document,
                    selected.first.node.selection_set.pos,
                ));
            }
            let target = manifest
                .models
                .get(&relationship.target_model)
                .ok_or_else(|| {
                    ClientCompileError::manifest(
                        "client.manifest.relationship_target",
                        format!(
                            "relationship aggregate `{}.{}` references absent target model `{}`",
                            model.id, aggregate.name, relationship.target_model
                        ),
                    )
                })?;
            let arguments = compile_arguments(
                &selected.first.node,
                &format!("{}.{}", model.id, aggregate.name),
                &aggregate.arguments,
                variables,
                used_variables,
                document,
            )?;
            let selection = compile_aggregate_object(
                &selected,
                target,
                &aggregate.semantics,
                &aggregate.arguments,
                &aggregate.dependencies,
                manifest,
                variables,
                used_variables,
                document,
                expander,
                depth + 1,
            )?;
            result.push(CompiledMember::Branch(Box::new(CompiledBranch {
                semantic: CompiledBranchSemantic::Aggregate,
                response_key: response_key.to_string(),
                field: aggregate.name.clone(),
                cardinality: Cardinality::One,
                nullable: true,
                arguments,
                dependencies: aggregate.dependencies.clone(),
                coverage: Some(complete_coverage()),
                maintenance: Some("revalidate".into()),
                selection,
                relationship_kind: None,
            })));
            continue;
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
                    selected.first.node.name.pos,
                )
            })?)
        };
        if !selected.first.node.arguments.is_empty() {
            return Err(source_error(
                "client.selection.field_arguments",
                format!("scalar field `{field_name}` must not have arguments"),
                document,
                selected.first.pos,
            ));
        }
        if !selected.selection_sets.is_empty() {
            return Err(source_error(
                "client.selection.scalar_nested",
                format!("scalar field `{field_name}` cannot have a nested selection"),
                document,
                selected.first.node.selection_set.pos,
            ));
        }
        result.push(CompiledMember::Scalar(match manifest_field {
            Some(field) => compiled_scalar(response_key, field, true),
            None => CompiledScalar {
                response_key: response_key.to_string(),
                field: "__typename".into(),
                codec: "string".into(),
                nullable: false,
                expose: true,
            },
        }));
    }
    if model.identity().is_some() {
        inject_wire_fields(&mut result, model, document, field.first.pos)?;
    }
    Ok(CompiledObject {
        typename: model.typename.clone(),
        storage: compiled_storage(model),
        members: result,
    })
}

fn compiled_storage(model: &ManifestModel) -> CompiledStorage {
    match model.identity() {
        Some(identity) => CompiledStorage::Normalized {
            model_id: model.id.clone(),
            identity_fields: identity.iter().map(|field| field.name.clone()).collect(),
        },
        None => CompiledStorage::Embedded,
    }
}

#[allow(clippy::too_many_arguments)]
fn compile_aggregate_object<'ast>(
    field: &MergedField<'ast>,
    model: &ManifestModel,
    semantics: &ManifestAggregateSemantics,
    arguments: &[ManifestArgument],
    dependencies: &[String],
    manifest: &ClientManifest,
    variables: &[CompiledVariable],
    used_variables: &mut BTreeSet<String>,
    document: &ClientDocument,
    expander: &mut FragmentExpander<'ast, '_>,
    depth: usize,
) -> Result<CompiledObject, ClientCompileError> {
    let selected_fields = expander.merge_object(
        &field.selection_sets,
        &semantics.wrapper_typename,
        depth,
        "aggregate field",
    )?;
    if selected_fields.is_empty() {
        return Err(source_error(
            "client.selection.empty",
            format!(
                "aggregate field `{}` must select `aggregate`, `nodes`, or `__typename`",
                field.first.node.name.node
            ),
            document,
            field.first.node.selection_set.pos,
        ));
    }
    let mut members = Vec::with_capacity(selected_fields.len());
    for selected in selected_fields {
        let response_key = selected.first.node.response_key().node.to_string();
        let field_name = selected.first.node.name.node.as_str();
        if !selected.first.node.arguments.is_empty() {
            return Err(source_error(
                "client.selection.field_arguments",
                format!("aggregate member `{field_name}` must not have arguments"),
                document,
                selected.first.pos,
            ));
        }
        match field_name {
            "aggregate" if semantics.count => {
                if selected.selection_sets.is_empty() {
                    return Err(source_error(
                        "client.selection.object_required",
                        "aggregate summary requires an object selection",
                        document,
                        selected.first.node.selection_set.pos,
                    ));
                }
                members.push(CompiledMember::Branch(Box::new(CompiledBranch {
                    semantic: CompiledBranchSemantic::AggregateFields,
                    response_key,
                    field: "aggregate".into(),
                    cardinality: Cardinality::One,
                    nullable: true,
                    arguments: BTreeMap::new(),
                    dependencies: dependencies.to_vec(),
                    coverage: Some(complete_coverage()),
                    maintenance: None,
                    selection: compile_aggregate_fields_object(
                        &selected,
                        semantics,
                        document,
                        expander,
                        depth + 1,
                    )?,
                    relationship_kind: None,
                })));
            }
            "nodes" if semantics.nodes => {
                if selected.selection_sets.is_empty() {
                    return Err(source_error(
                        "client.selection.object_required",
                        "aggregate nodes require an object selection",
                        document,
                        selected.first.node.selection_set.pos,
                    ));
                }
                members.push(CompiledMember::Branch(Box::new(CompiledBranch {
                    semantic: CompiledBranchSemantic::AggregateNodes,
                    response_key,
                    field: "nodes".into(),
                    cardinality: Cardinality::Many,
                    nullable: false,
                    arguments: BTreeMap::new(),
                    dependencies: dependencies.to_vec(),
                    coverage: compile_coverage(
                        Some(&semantics.nodes_pagination),
                        arguments,
                        &format!("{}.nodes", field.first.node.name.node),
                        document,
                        selected.first.pos,
                    )?,
                    maintenance: None,
                    selection: compile_model_object(
                        &selected,
                        model,
                        manifest,
                        variables,
                        used_variables,
                        document,
                        expander,
                        depth + 1,
                    )?,
                    relationship_kind: None,
                })));
            }
            "__typename" => {
                if !selected.selection_sets.is_empty() {
                    return Err(source_error(
                        "client.selection.scalar_nested",
                        "scalar field `__typename` cannot have a nested selection",
                        document,
                        selected.first.node.selection_set.pos,
                    ));
                }
                members.push(CompiledMember::Scalar(CompiledScalar {
                    response_key,
                    field: "__typename".into(),
                    codec: "string".into(),
                    nullable: false,
                    expose: true,
                }));
            }
            "aggregate" | "nodes" => {
                return Err(source_error(
                    "client.selection.aggregate_denied",
                    format!(
                        "aggregate member `{field_name}` is absent from the selected manifest semantics"
                    ),
                    document,
                    selected.first.node.name.pos,
                ));
            }
            _ => {
                return Err(source_error(
                    "client.selection.denied_or_unknown",
                    format!(
                        "field `{field_name}` is absent from aggregate type `{}`",
                        semantics.wrapper_typename
                    ),
                    document,
                    selected.first.node.name.pos,
                ));
            }
        }
    }
    Ok(CompiledObject {
        typename: semantics.wrapper_typename.clone(),
        storage: CompiledStorage::Embedded,
        members,
    })
}

fn compile_aggregate_fields_object<'ast>(
    field: &MergedField<'ast>,
    semantics: &ManifestAggregateSemantics,
    document: &ClientDocument,
    expander: &mut FragmentExpander<'ast, '_>,
    depth: usize,
) -> Result<CompiledObject, ClientCompileError> {
    let selected_fields = expander.merge_object(
        &field.selection_sets,
        &semantics.fields_typename,
        depth,
        "aggregate summary field",
    )?;
    if selected_fields.is_empty() {
        return Err(source_error(
            "client.selection.empty",
            "aggregate summary must select at least one field",
            document,
            field.first.node.selection_set.pos,
        ));
    }
    let mut members = Vec::with_capacity(selected_fields.len());
    for selected in selected_fields {
        let response_key = selected.first.node.response_key().node.to_string();
        let field_name = selected.first.node.name.node.as_str();
        if !selected.first.node.arguments.is_empty() || !selected.selection_sets.is_empty() {
            return Err(source_error(
                "client.selection.aggregate_metric_shape",
                format!("aggregate metric `{field_name}` must be a scalar leaf"),
                document,
                selected.first.pos,
            ));
        }
        match field_name {
            "count" if semantics.count => members.push(CompiledMember::Scalar(CompiledScalar {
                response_key,
                field: "count".into(),
                codec: "int32".into(),
                nullable: false,
                expose: true,
            })),
            "__typename" => members.push(CompiledMember::Scalar(CompiledScalar {
                response_key,
                field: "__typename".into(),
                codec: "string".into(),
                nullable: false,
                expose: true,
            })),
            "sum" | "avg" | "min" | "max" => {
                return Err(source_error(
                    "client.selection.aggregate_metric_unsupported",
                    format!(
                        "aggregate metric `{field_name}` needs a typed metric-object contract before it can be compiled"
                    ),
                    document,
                    selected.first.node.name.pos,
                ));
            }
            _ => {
                return Err(source_error(
                    "client.selection.denied_or_unknown",
                    format!(
                        "field `{field_name}` is absent from aggregate summary type `{}`",
                        semantics.fields_typename
                    ),
                    document,
                    selected.first.node.name.pos,
                ));
            }
        }
    }
    Ok(CompiledObject {
        typename: semantics.fields_typename.clone(),
        storage: CompiledStorage::Embedded,
        members,
    })
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
    members: &mut Vec<CompiledMember>,
    model: &ManifestModel,
    document: &ClientDocument,
    position: Pos,
) -> Result<(), ClientCompileError> {
    let identity = model
        .identity()
        .expect("embedded model rejected before injection");
    let mut response_keys = members
        .iter()
        .map(|member| match member {
            CompiledMember::Scalar(scalar) => scalar.response_key.clone(),
            CompiledMember::Branch(branch) => branch.response_key.clone(),
        })
        .collect::<BTreeSet<_>>();
    for identity_field in identity {
        if members.iter().any(|member| match member {
            CompiledMember::Scalar(scalar) => scalar.field == identity_field.name,
            CompiledMember::Branch(_) => false,
        }) {
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
        members.push(CompiledMember::Scalar(compiled_scalar(
            &response_key,
            field,
            false,
        )));
    }
    if !members.iter().any(|member| match member {
        CompiledMember::Scalar(scalar) => scalar.field == "__typename",
        CompiledMember::Branch(_) => false,
    }) {
        let response_key = allocate_wire_alias("typename", &mut response_keys);
        members.push(CompiledMember::Scalar(CompiledScalar {
            response_key,
            field: "__typename".into(),
            codec: "string".into(),
            nullable: false,
            expose: false,
        }));
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
    pagination: Option<&ManifestPagination>,
    arguments: &[ManifestArgument],
    owner: &str,
    document: &ClientDocument,
    position: Pos,
) -> Result<Option<CompiledCoverage>, ClientCompileError> {
    let Some(pagination) = pagination else {
        return Ok(Some(complete_coverage()));
    };
    if pagination.kind != "offset" || pagination.coverage != "window" {
        return Err(source_error(
            "client.pagination.unsupported",
            format!(
                "field `{owner}` uses unsupported pagination contract kind=`{}` coverage=`{}`",
                pagination.kind, pagination.coverage
            ),
            document,
            position,
        ));
    }
    Ok(Some(CompiledCoverage {
        kind: "offset".into(),
        offset_argument: arguments
            .iter()
            .find(|argument| argument.kind == ManifestArgumentKind::Offset)
            .map(|argument| argument.name.clone()),
        limit_argument: arguments
            .iter()
            .find(|argument| argument.kind == ManifestArgumentKind::Limit)
            .map(|argument| argument.name.clone()),
        default_limit: Some(pagination.default_limit),
        max_limit: Some(pagination.max_limit),
    }))
}

fn complete_coverage() -> CompiledCoverage {
    CompiledCoverage {
        kind: "complete".into(),
        offset_argument: None,
        limit_argument: None,
        default_limit: None,
        max_limit: None,
    }
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
    let arguments = render_compiled_arguments(&root.arguments);
    let root_prefix = if root.response_key == root.field {
        root.field.clone()
    } else {
        format!("{}: {}", root.response_key, root.field)
    };
    let mut lines = vec![format!("{operation_type} {name}{variable_definitions} {{")];
    lines.push(format!("  {root_prefix}{arguments} {{"));
    render_object_selection(&mut lines, &root.selection, 4);
    lines.push("  }".into());
    lines.push("}".into());
    Ok(format!("{}\n", lines.join("\n")))
}

fn render_object_selection(lines: &mut Vec<String>, object: &CompiledObject, indent: usize) {
    let padding = " ".repeat(indent);
    for member in &object.members {
        match member {
            CompiledMember::Scalar(field) => {
                let prefix = if field.response_key == field.field {
                    field.field.clone()
                } else {
                    format!("{}: {}", field.response_key, field.field)
                };
                lines.push(format!("{padding}{prefix}"));
            }
            CompiledMember::Branch(branch) => {
                let prefix = if branch.response_key == branch.field {
                    branch.field.clone()
                } else {
                    format!("{}: {}", branch.response_key, branch.field)
                };
                let arguments = render_compiled_arguments(&branch.arguments);
                lines.push(format!("{padding}{prefix}{arguments} {{"));
                render_object_selection(lines, &branch.selection, indent + 2);
                lines.push(format!("{padding}}}"));
            }
        }
    }
}

fn render_compiled_arguments(arguments: &BTreeMap<String, CompiledArgument>) -> String {
    if arguments.is_empty() {
        return String::new();
    }
    format!(
        "({})",
        arguments
            .iter()
            .map(|(name, value)| format!(
                "{name}: {}",
                match value {
                    CompiledArgument::Literal { wire, .. } => wire.clone(),
                    CompiledArgument::Variable(variable) => format!("${variable}"),
                }
            ))
            .collect::<Vec<_>>()
            .join(", ")
    )
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
