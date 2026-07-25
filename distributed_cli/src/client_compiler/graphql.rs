use std::collections::{BTreeMap, BTreeSet, HashMap};

use async_graphql_parser::types::{
    BaseType, Directive, DocumentOperations, Field, FragmentDefinition, OperationDefinition,
    OperationType, Selection, SelectionSet, Type,
};
use async_graphql_parser::{parse_query, Pos, Positioned};
use async_graphql_value::{Name, Value};
use base64::Engine as _;
use serde::Serialize;
use serde_json::{Map as JsonMap, Value as JsonValue};

use super::manifest::{
    hash_bytes, ClientManifest, ManifestAggregateSemantics, ManifestArgument, ManifestArgumentKind,
    ManifestExecutionLimits, ManifestField, ManifestFilterExpr, ManifestFilterInput,
    ManifestFilterSemantics, ManifestModel, ManifestOrderSemantics, ManifestPagination,
    ManifestRelationship, ManifestRelationshipKeyMapping, ManifestRelationshipKind,
    ManifestRelationshipMaintenance, ManifestRoot, ManifestRowPolicy, RootKind, RootOperation,
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
    pub(crate) source_line: usize,
    pub(crate) source_column: usize,
    pub(crate) module_path: String,
    pub(crate) export_name: String,
    pub(crate) query_document: String,
    pub(crate) query_hash: String,
    pub(crate) live: Option<CompiledLiveOperation>,
    pub(crate) variables: Vec<CompiledVariable>,
    pub(crate) variable_codec: CompiledVariableCodec,
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

#[derive(Clone, Debug, Serialize)]
pub(crate) struct CompiledVariableCodec {
    pub(crate) version: u32,
    pub(crate) limits: CompiledVariableCodecLimits,
    pub(crate) variables: BTreeMap<String, CompiledInputType>,
    pub(crate) inputs: BTreeMap<String, CompiledInputDefinition>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct CompiledVariableCodecLimits {
    #[serde(rename = "maxDepth")]
    pub(crate) max_depth: u64,
    #[serde(rename = "maxBoolWidth")]
    pub(crate) max_bool_width: u64,
    #[serde(rename = "maxInList")]
    pub(crate) max_in_list: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum CompiledInputType {
    Scalar {
        scalar: String,
        codec: String,
        nullable: bool,
    },
    Enum {
        name: String,
        values: Vec<String>,
        nullable: bool,
    },
    Input {
        name: String,
        nullable: bool,
        #[serde(rename = "filterBaseDepth", skip_serializing_if = "Option::is_none")]
        filter_base_depth: Option<u64>,
    },
    List {
        nullable: bool,
        #[serde(rename = "maxItems", skip_serializing_if = "Option::is_none")]
        max_items: Option<u64>,
        item: Box<CompiledInputType>,
    },
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum CompiledInputDefinition {
    Filter {
        model: String,
        fields: Vec<CompiledFilterInputField>,
        relationships: Vec<CompiledFilterInputRelationship>,
    },
    Order {
        model: String,
        fields: Vec<CompiledOrderInputField>,
        values: Vec<String>,
    },
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct CompiledFilterInputField {
    pub(crate) field: String,
    pub(crate) scalar: String,
    pub(crate) codec: String,
    pub(crate) nullable: bool,
    pub(crate) operators: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct CompiledOrderInputField {
    pub(crate) field: String,
    pub(crate) scalar: String,
    pub(crate) codec: String,
    pub(crate) nullable: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct CompiledFilterInputRelationship {
    pub(crate) field: String,
    pub(crate) target: CompiledFilterInputTarget,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum CompiledFilterInputTarget {
    Input { name: String },
    Opaque,
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
    pub(crate) filter: Option<CompiledFilterPlan>,
    pub(crate) order: Option<CompiledOrderPlan>,
    pub(crate) pagination: Option<CompiledPaginationPlan>,
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
    pub(crate) filter: Option<CompiledFilterPlan>,
    pub(crate) order: Option<CompiledOrderPlan>,
    pub(crate) pagination: Option<CompiledPaginationPlan>,
    pub(crate) relationship: Option<CompiledRelationshipPlan>,
    pub(crate) selection: CompiledObject,
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
    List(Vec<CompiledArgument>),
    Object(BTreeMap<String, CompiledArgument>),
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
pub(crate) struct CompiledFilterPlan {
    pub(crate) input: Option<CompiledArgument>,
    pub(crate) fields: Vec<CompiledFilterField>,
    pub(crate) relationships: Vec<CompiledRelationshipPlan>,
    pub(crate) row_policy: ManifestRowPolicy,
    variable_constraints: BTreeMap<String, VariableUseConstraint>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct VariableUseConstraint {
    filter_base_depth: Option<u64>,
    max_items: Option<u64>,
    item: Option<Box<VariableUseConstraint>>,
}

impl VariableUseConstraint {
    fn filter(base_depth: u64) -> Self {
        Self {
            filter_base_depth: Some(base_depth),
            ..Self::default()
        }
    }

    fn list(max_items: u64, item: Option<Self>) -> Self {
        Self {
            max_items: Some(max_items),
            item: item.map(Box::new),
            ..Self::default()
        }
    }

    fn intersect(&mut self, other: &Self) {
        self.filter_base_depth = match (self.filter_base_depth, other.filter_base_depth) {
            (Some(left), Some(right)) => Some(left.max(right)),
            (left, right) => left.or(right),
        };
        self.max_items = match (self.max_items, other.max_items) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (left, right) => left.or(right),
        };
        match (&mut self.item, &other.item) {
            (Some(left), Some(right)) => left.intersect(right),
            (None, Some(right)) => self.item = Some(right.clone()),
            _ => {}
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CompiledFilterField {
    pub(crate) name: String,
    pub(crate) scalar: String,
    pub(crate) codec: String,
    pub(crate) nullable: bool,
    pub(crate) operators: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct CompiledRelationshipPlan {
    pub(crate) field: String,
    pub(crate) target_model: String,
    pub(crate) kind: ManifestRelationshipKind,
    pub(crate) key_mapping: ManifestRelationshipKeyMapping,
    pub(crate) maintenance: ManifestRelationshipMaintenance,
    pub(crate) dependencies: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct CompiledOrderPlan {
    pub(crate) input: Option<CompiledArgument>,
    pub(crate) fields: Vec<CompiledOrderField>,
    pub(crate) identity: Vec<CompiledOrderField>,
}

#[derive(Clone, Debug)]
pub(crate) struct CompiledOrderField {
    pub(crate) name: String,
    pub(crate) scalar: String,
    pub(crate) codec: String,
    pub(crate) nullable: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct CompiledPaginationPlan {
    pub(crate) kind: String,
    pub(crate) insert: String,
    pub(crate) delete: String,
    pub(crate) reorder: String,
    pub(crate) stable_update: String,
}

type CompiledQueryPlans = (
    Option<CompiledFilterPlan>,
    Option<CompiledOrderPlan>,
    Option<CompiledPaginationPlan>,
);

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
        model,
        manifest,
        &variables,
        &mut used_variables,
        document,
    )?;
    let mut selection = match root_manifest.kind {
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
                &compiled_arguments,
                root_manifest.filter.as_ref(),
                root_manifest.order.as_ref(),
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
    if root_manifest.kind == RootKind::List {
        let dependencies = query_plan_field_dependencies(
            model,
            root_manifest.filter.as_ref(),
            root_manifest.order.as_ref(),
            &root_manifest.arguments,
            &compiled_arguments,
        );
        inject_dependency_fields(
            &mut selection,
            model,
            &dependencies,
            document,
            root_field.first.pos,
        )?;
    }
    validate_used_variables(&variables, &used_variables, document, operation.pos)?;
    expander.reject_unused_fragments()?;

    let cardinality = match root_manifest.kind {
        RootKind::List => Cardinality::Many,
        RootKind::ByPk => Cardinality::One,
        RootKind::Aggregate => Cardinality::One,
    };
    let coverage = match root_manifest.kind {
        RootKind::Aggregate => Some(complete_coverage()),
        RootKind::List | RootKind::ByPk => compile_coverage(
            root_manifest.pagination.as_ref(),
            &root_manifest.arguments,
            &root_manifest.name,
            document,
            root_field.first.pos,
        )?,
    };
    let (filter, order, pagination) = compile_query_plans(
        model,
        root_manifest.filter.as_ref(),
        root_manifest.order.as_ref(),
        &root_manifest.arguments,
        &compiled_arguments,
        &variables,
        manifest,
        document,
        &root_field.first.node,
        coverage.as_ref(),
        0,
        root_manifest.kind == RootKind::List,
    )?;
    let root = CompiledRoot {
        response_key: root_field.first.node.response_key().node.to_string(),
        field: root_name.to_string(),
        cardinality,
        nullable: matches!(root_manifest.kind, RootKind::ByPk | RootKind::Aggregate),
        arguments: compiled_arguments,
        dependencies: root_manifest.dependencies.clone(),
        coverage,
        filter,
        order,
        pagination,
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
    let variable_constraints = operation_variable_constraints(&root);
    let variable_codec = compile_variable_codec(&variables, manifest, &variable_constraints)?;
    let module_stem = module_stem(&name);
    Ok(CompiledOperation {
        name: name.clone(),
        source_path: document.path.clone(),
        source_line: operation.pos.line.max(1),
        source_column: operation.pos.column.max(1),
        module_path: format!("operations/{module_stem}.ts"),
        export_name: format!("Operation_{name}"),
        query_document,
        query_hash,
        live,
        variables,
        variable_codec,
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
                        .relationship
                        .as_ref()
                        .expect("relationship branches retain their compiled descriptor")
                        .kind
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

#[derive(Clone, Copy)]
struct FilterInputCandidate<'a> {
    model: &'a ManifestModel,
    semantics: &'a ManifestFilterInput,
}

#[derive(Clone, Copy)]
struct OrderInputCandidate<'a> {
    model: &'a ManifestModel,
    semantics: &'a ManifestOrderSemantics,
}

fn compile_variable_codec(
    variables: &[CompiledVariable],
    manifest: &ClientManifest,
    constraints: &BTreeMap<String, VariableUseConstraint>,
) -> Result<CompiledVariableCodec, ClientCompileError> {
    let mut filters = BTreeMap::<String, FilterInputCandidate<'_>>::new();
    let mut orders = BTreeMap::<String, OrderInputCandidate<'_>>::new();

    for model in manifest.models.values() {
        register_filter_input(
            &model.filter_input.type_name,
            model,
            &model.filter_input,
            &mut filters,
        )?;
    }

    for root in manifest.roots.values() {
        let model = manifest.models.get(&root.model).ok_or_else(|| {
            ClientCompileError::manifest(
                "client.manifest.root_model",
                format!(
                    "root `{}` references absent model `{}`",
                    root.name, root.model
                ),
            )
        })?;
        register_order_input_candidate(&root.arguments, root.order.as_ref(), model, &mut orders)?;
    }

    for source in manifest.models.values() {
        for relationship in &source.relationships {
            let target = manifest
                .models
                .get(&relationship.target_model)
                .ok_or_else(|| {
                    ClientCompileError::manifest(
                        "client.manifest.relationship_target",
                        format!(
                            "relationship `{}.{}` references absent model `{}`",
                            source.id, relationship.name, relationship.target_model
                        ),
                    )
                })?;
            register_order_input_candidate(
                &relationship.arguments,
                relationship.order.as_ref(),
                target,
                &mut orders,
            )?;
            if let Some(aggregate) = &relationship.aggregate {
                register_order_input_candidate(
                    &aggregate.arguments,
                    relationship.order.as_ref(),
                    target,
                    &mut orders,
                )?;
            }
        }
    }

    if let Some(name) = filters.keys().find(|name| orders.contains_key(*name)) {
        return Err(ClientCompileError::manifest(
            "client.variable.input_type_conflict",
            format!(
                "selected manifest uses input type `{name}` for both filter and order contracts"
            ),
        ));
    }

    let order_values = orders
        .values()
        .next()
        .map(|candidate| candidate.semantics.values.clone())
        .unwrap_or_default();
    if orders
        .values()
        .any(|candidate| candidate.semantics.values != order_values)
    {
        return Err(ClientCompileError::manifest(
            "client.variable.order_enum_conflict",
            "selected order input contracts disagree on the direction enum",
        ));
    }

    let mut inputs = BTreeMap::new();
    let mut visiting = BTreeSet::new();
    let mut compiled_variables = BTreeMap::new();
    for variable in variables {
        let input_type = compile_variable_input_type(
            &variable.graphql_type,
            manifest,
            &filters,
            &orders,
            &order_values,
            &mut inputs,
            &mut visiting,
            constraints.get(&variable.name),
        )?;
        compiled_variables.insert(variable.name.clone(), input_type);
    }
    Ok(CompiledVariableCodec {
        version: 2,
        limits: CompiledVariableCodecLimits {
            max_depth: manifest.execution.max_depth,
            max_bool_width: manifest.execution.max_bool_width,
            max_in_list: manifest.execution.max_in_list,
        },
        variables: compiled_variables,
        inputs,
    })
}

fn operation_variable_constraints(root: &CompiledRoot) -> BTreeMap<String, VariableUseConstraint> {
    let mut constraints = BTreeMap::new();
    merge_filter_constraints(root.filter.as_ref(), &mut constraints);
    merge_object_variable_constraints(&root.selection, &mut constraints);
    constraints
}

fn merge_object_variable_constraints(
    object: &CompiledObject,
    constraints: &mut BTreeMap<String, VariableUseConstraint>,
) {
    for member in &object.members {
        let CompiledMember::Branch(branch) = member else {
            continue;
        };
        merge_filter_constraints(branch.filter.as_ref(), constraints);
        merge_object_variable_constraints(&branch.selection, constraints);
    }
}

fn merge_filter_constraints(
    filter: Option<&CompiledFilterPlan>,
    constraints: &mut BTreeMap<String, VariableUseConstraint>,
) {
    let Some(filter) = filter else {
        return;
    };
    for (name, constraint) in &filter.variable_constraints {
        constraints
            .entry(name.clone())
            .and_modify(|existing| existing.intersect(constraint))
            .or_insert_with(|| constraint.clone());
    }
}

fn register_order_input_candidate<'a>(
    arguments: &[ManifestArgument],
    order: Option<&'a ManifestOrderSemantics>,
    model: &'a ManifestModel,
    orders: &mut BTreeMap<String, OrderInputCandidate<'a>>,
) -> Result<(), ClientCompileError> {
    if let Some(semantics) = order {
        if let Some(argument) = arguments
            .iter()
            .find(|argument| argument.kind == ManifestArgumentKind::Order)
        {
            register_order_input(&argument.type_name, model, semantics, orders)?;
        }
    }
    Ok(())
}

fn register_filter_input<'a>(
    name: &str,
    model: &'a ManifestModel,
    semantics: &'a ManifestFilterInput,
    candidates: &mut BTreeMap<String, FilterInputCandidate<'a>>,
) -> Result<(), ClientCompileError> {
    if let Some(existing) = candidates.get(name) {
        if existing.model.id != model.id || existing.semantics != semantics {
            return Err(ClientCompileError::manifest(
                "client.variable.input_type_conflict",
                format!("filter input type `{name}` has multiple selected structural contracts"),
            ));
        }
        return Ok(());
    }
    candidates.insert(name.to_string(), FilterInputCandidate { model, semantics });
    Ok(())
}

fn register_order_input<'a>(
    name: &str,
    model: &'a ManifestModel,
    semantics: &'a ManifestOrderSemantics,
    candidates: &mut BTreeMap<String, OrderInputCandidate<'a>>,
) -> Result<(), ClientCompileError> {
    if let Some(existing) = candidates.get(name) {
        if existing.model.id != model.id || existing.semantics != semantics {
            return Err(ClientCompileError::manifest(
                "client.variable.input_type_conflict",
                format!("order input type `{name}` has multiple selected structural contracts"),
            ));
        }
        return Ok(());
    }
    candidates.insert(name.to_string(), OrderInputCandidate { model, semantics });
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn compile_variable_input_type(
    graphql_type: &Type,
    manifest: &ClientManifest,
    filters: &BTreeMap<String, FilterInputCandidate<'_>>,
    orders: &BTreeMap<String, OrderInputCandidate<'_>>,
    order_values: &[String],
    inputs: &mut BTreeMap<String, CompiledInputDefinition>,
    visiting: &mut BTreeSet<String>,
    constraint: Option<&VariableUseConstraint>,
) -> Result<CompiledInputType, ClientCompileError> {
    let nullable = graphql_type.nullable;
    match &graphql_type.base {
        BaseType::List(item) => {
            if constraint.is_some_and(|constraint| constraint.filter_base_depth.is_some()) {
                return Err(ClientCompileError::manifest(
                    "client.variable.constraint_type",
                    "filterBaseDepth cannot apply to a list variable",
                ));
            }
            Ok(CompiledInputType::List {
                nullable,
                max_items: constraint.and_then(|constraint| constraint.max_items),
                item: Box::new(compile_variable_input_type(
                    item,
                    manifest,
                    filters,
                    orders,
                    order_values,
                    inputs,
                    visiting,
                    constraint.and_then(|constraint| constraint.item.as_deref()),
                )?),
            })
        }
        BaseType::Named(name) => {
            let name = name.as_str();
            if let Some(codec) = manifest.scalar_codecs.get(name) {
                require_leaf_constraint(name, constraint)?;
                return Ok(CompiledInputType::Scalar {
                    scalar: name.to_string(),
                    codec: codec.clone(),
                    nullable,
                });
            }
            if filters.contains_key(name) {
                if constraint.is_some_and(|constraint| {
                    constraint.max_items.is_some() || constraint.item.is_some()
                }) {
                    return Err(ClientCompileError::manifest(
                        "client.variable.constraint_type",
                        format!("filter input `{name}` received list constraints"),
                    ));
                }
                compile_filter_input_definition(name, filters, inputs, visiting)?;
                return Ok(CompiledInputType::Input {
                    name: name.to_string(),
                    nullable,
                    filter_base_depth: constraint
                        .and_then(|constraint| constraint.filter_base_depth),
                });
            }
            if orders.contains_key(name) {
                require_leaf_constraint(name, constraint)?;
                compile_order_input_definition(name, orders, inputs)?;
                return Ok(CompiledInputType::Input {
                    name: name.to_string(),
                    nullable,
                    filter_base_depth: None,
                });
            }
            if name == "order_by" && !order_values.is_empty() {
                require_leaf_constraint(name, constraint)?;
                return Ok(CompiledInputType::Enum {
                    name: name.to_string(),
                    values: order_values.to_vec(),
                    nullable,
                });
            }
            Err(ClientCompileError::manifest(
                "client.variable.input_type_unsupported",
                format!(
                    "variable input type `{name}` has no compiler-owned scalar, filter, order, or enum contract"
                ),
            ))
        }
    }
}

fn require_leaf_constraint(
    name: &str,
    constraint: Option<&VariableUseConstraint>,
) -> Result<(), ClientCompileError> {
    if constraint.is_none_or(|constraint| {
        constraint.filter_base_depth.is_none()
            && constraint.max_items.is_none()
            && constraint.item.is_none()
    }) {
        return Ok(());
    }
    Err(ClientCompileError::manifest(
        "client.variable.constraint_type",
        format!("variable type `{name}` received incompatible input constraints"),
    ))
}

fn compile_filter_input_definition(
    name: &str,
    candidates: &BTreeMap<String, FilterInputCandidate<'_>>,
    inputs: &mut BTreeMap<String, CompiledInputDefinition>,
    visiting: &mut BTreeSet<String>,
) -> Result<(), ClientCompileError> {
    if inputs.contains_key(name) || !visiting.insert(name.to_string()) {
        return Ok(());
    }
    let candidate = *candidates.get(name).ok_or_else(|| {
        ClientCompileError::manifest(
            "client.variable.filter_input",
            format!("filter input type `{name}` has no selected contract"),
        )
    })?;
    let fields = candidate
        .semantics
        .fields
        .iter()
        .map(|semantics| {
            let field = candidate.model.field(&semantics.name).ok_or_else(|| {
                ClientCompileError::manifest(
                    "client.variable.filter_field",
                    format!(
                        "filter input `{name}` references absent field `{}.{}`",
                        candidate.model.id, semantics.name
                    ),
                )
            })?;
            Ok(CompiledFilterInputField {
                field: field.name.clone(),
                scalar: field.scalar.clone(),
                codec: field.codec.clone(),
                nullable: field.nullable,
                operators: semantics.operators.clone(),
            })
        })
        .collect::<Result<Vec<_>, ClientCompileError>>()?;

    let mut relationships = Vec::with_capacity(candidate.semantics.relationships.len());
    for relationship_input in &candidate.semantics.relationships {
        candidate
            .model
            .relationship(&relationship_input.field)
            .ok_or_else(|| {
                ClientCompileError::manifest(
                    "client.variable.filter_relationship",
                    format!(
                        "filter input `{name}` references absent relationship `{}.{}`",
                        candidate.model.id, relationship_input.field
                    ),
                )
            })?;
        let target = candidates
            .contains_key(&relationship_input.target_type)
            .then(|| {
                compile_filter_input_definition(
                    &relationship_input.target_type,
                    candidates,
                    inputs,
                    visiting,
                )?;
                Ok(CompiledFilterInputTarget::Input {
                    name: relationship_input.target_type.clone(),
                })
            })
            .transpose()?
            .unwrap_or(CompiledFilterInputTarget::Opaque);
        relationships.push(CompiledFilterInputRelationship {
            field: relationship_input.field.clone(),
            target,
        });
    }
    inputs.insert(
        name.to_string(),
        CompiledInputDefinition::Filter {
            model: candidate.model.id.clone(),
            fields,
            relationships,
        },
    );
    visiting.remove(name);
    Ok(())
}

fn compile_order_input_definition(
    name: &str,
    candidates: &BTreeMap<String, OrderInputCandidate<'_>>,
    inputs: &mut BTreeMap<String, CompiledInputDefinition>,
) -> Result<(), ClientCompileError> {
    if inputs.contains_key(name) {
        return Ok(());
    }
    let candidate = *candidates.get(name).ok_or_else(|| {
        ClientCompileError::manifest(
            "client.variable.order_input",
            format!("order input type `{name}` has no selected contract"),
        )
    })?;
    let fields = candidate
        .semantics
        .fields
        .iter()
        .map(|field_name| {
            let field = candidate.model.field(field_name).ok_or_else(|| {
                ClientCompileError::manifest(
                    "client.variable.order_field",
                    format!(
                        "order input `{name}` references absent field `{}.{field_name}`",
                        candidate.model.id
                    ),
                )
            })?;
            Ok(CompiledOrderInputField {
                field: field.name.clone(),
                scalar: field.scalar.clone(),
                codec: field.codec.clone(),
                nullable: field.nullable,
            })
        })
        .collect::<Result<Vec<_>, ClientCompileError>>()?;
    inputs.insert(
        name.to_string(),
        CompiledInputDefinition::Order {
            model: candidate.model.id.clone(),
            fields,
            values: candidate.semantics.values.clone(),
        },
    );
    Ok(())
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

#[allow(clippy::too_many_arguments)]
fn compile_arguments(
    field: &Field,
    owner: &str,
    allowed_arguments: &[ManifestArgument],
    model: &ManifestModel,
    manifest: &ClientManifest,
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
                let literal =
                    canonicalize_argument_literal(literal, manifest_argument, model, manifest);
                validate_literal(&literal, manifest_argument, document, value.pos)?;
                compile_argument_source(
                    &literal,
                    name_string,
                    &variables,
                    used_variables,
                    document,
                    value.pos,
                )?
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
    let mut relationship_source_fields = BTreeSet::new();
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
                target,
                manifest,
                variables,
                used_variables,
                document,
            )?;
            let relationship_plan = compiled_relationship_plan(relationship);
            let (local_keys, remote_keys) = relationship_key_fields(&relationship_plan.key_mapping);
            relationship_source_fields.extend(local_keys.iter().cloned());
            let mut selection = compile_model_object(
                &selected,
                target,
                manifest,
                variables,
                used_variables,
                document,
                expander,
                depth + 1,
            )?;
            let mut dependencies = BTreeSet::new();
            if relationship.list {
                dependencies.extend(query_plan_field_dependencies(
                    target,
                    relationship.filter.as_ref(),
                    relationship.order.as_ref(),
                    &relationship.arguments,
                    &arguments,
                ));
            }
            dependencies.extend(remote_keys.iter().cloned());
            if !dependencies.is_empty() {
                inject_dependency_fields(
                    &mut selection,
                    target,
                    &dependencies,
                    document,
                    selected.first.pos,
                )?;
            }
            let cardinality = if relationship.list {
                Cardinality::Many
            } else {
                Cardinality::One
            };
            let coverage = compile_coverage(
                relationship.pagination.as_ref(),
                &relationship.arguments,
                &format!("{}.{}", model.id, relationship.name),
                document,
                selected.first.pos,
            )?;
            let (filter, order, pagination) = compile_query_plans(
                target,
                relationship.filter.as_ref(),
                relationship.order.as_ref(),
                &relationship.arguments,
                &arguments,
                variables,
                manifest,
                document,
                &selected.first.node,
                coverage.as_ref(),
                filter_depth_from_selection(depth, 1),
                relationship.list,
            )?;
            result.push(CompiledMember::Branch(Box::new(CompiledBranch {
                semantic: CompiledBranchSemantic::Relationship,
                response_key: response_key.to_string(),
                field: relationship.name.clone(),
                cardinality,
                nullable: relationship.nullable,
                arguments,
                dependencies: relationship.dependencies.clone(),
                coverage,
                filter,
                order,
                pagination,
                relationship: Some(relationship_plan),
                selection,
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
                target,
                manifest,
                variables,
                used_variables,
                document,
            )?;
            let selection = compile_aggregate_object(
                &selected,
                target,
                &aggregate.semantics,
                &aggregate.arguments,
                &arguments,
                relationship.filter.as_ref(),
                relationship.order.as_ref(),
                &aggregate.dependencies,
                manifest,
                variables,
                used_variables,
                document,
                expander,
                depth + 1,
            )?;
            let (filter, order, pagination) = compile_query_plans(
                target,
                relationship.filter.as_ref(),
                relationship.order.as_ref(),
                &aggregate.arguments,
                &arguments,
                variables,
                manifest,
                document,
                &selected.first.node,
                None,
                filter_depth_from_selection(depth, 1),
                false,
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
                filter,
                order,
                pagination,
                relationship: None,
                selection,
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
    let mut object = CompiledObject {
        typename: model.typename.clone(),
        storage: compiled_storage(model),
        members: result,
    };
    inject_dependency_fields(
        &mut object,
        model,
        &relationship_source_fields,
        document,
        field.first.pos,
    )?;
    Ok(object)
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

fn compiled_relationship_plan(relationship: &ManifestRelationship) -> CompiledRelationshipPlan {
    let maintenance = match &relationship.key_mapping {
        ManifestRelationshipKeyMapping::Direct { .. }
        | ManifestRelationshipKeyMapping::Through { .. } => relationship.maintenance,
        ManifestRelationshipKeyMapping::ThroughOpaque { .. }
        | ManifestRelationshipKeyMapping::Embedded => ManifestRelationshipMaintenance::Revalidate,
    };
    CompiledRelationshipPlan {
        field: relationship.name.clone(),
        target_model: relationship.target_model.clone(),
        kind: relationship.kind,
        key_mapping: relationship.key_mapping.clone(),
        maintenance,
        dependencies: relationship.dependencies.clone(),
    }
}

fn relationship_key_fields(mapping: &ManifestRelationshipKeyMapping) -> (&[String], &[String]) {
    match mapping {
        ManifestRelationshipKeyMapping::Direct { local, remote }
        | ManifestRelationshipKeyMapping::Through { local, remote, .. }
        | ManifestRelationshipKeyMapping::ThroughOpaque { local, remote, .. } => (local, remote),
        ManifestRelationshipKeyMapping::Embedded => (&[], &[]),
    }
}

#[allow(clippy::too_many_arguments)]
fn compile_aggregate_object<'ast>(
    field: &MergedField<'ast>,
    model: &ManifestModel,
    semantics: &ManifestAggregateSemantics,
    arguments: &[ManifestArgument],
    compiled_arguments: &BTreeMap<String, CompiledArgument>,
    filter_semantics: Option<&ManifestFilterSemantics>,
    order_semantics: Option<&ManifestOrderSemantics>,
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
                    filter: None,
                    order: None,
                    pagination: None,
                    relationship: None,
                    selection: compile_aggregate_fields_object(
                        &selected,
                        semantics,
                        document,
                        expander,
                        depth + 1,
                    )?,
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
                let coverage = compile_coverage(
                    Some(&semantics.nodes_pagination),
                    arguments,
                    &format!("{}.nodes", field.first.node.name.node),
                    document,
                    selected.first.pos,
                )?;
                let mut selection = compile_model_object(
                    &selected,
                    model,
                    manifest,
                    variables,
                    used_variables,
                    document,
                    expander,
                    depth + 1,
                )?;
                let dependency_fields = query_plan_field_dependencies(
                    model,
                    filter_semantics,
                    order_semantics,
                    arguments,
                    compiled_arguments,
                );
                inject_dependency_fields(
                    &mut selection,
                    model,
                    &dependency_fields,
                    document,
                    selected.first.pos,
                )?;
                let (filter, order, pagination) = compile_query_plans(
                    model,
                    filter_semantics,
                    order_semantics,
                    arguments,
                    compiled_arguments,
                    variables,
                    manifest,
                    document,
                    &field.first.node,
                    coverage.as_ref(),
                    filter_depth_from_selection(depth, 2),
                    true,
                )?;
                members.push(CompiledMember::Branch(Box::new(CompiledBranch {
                    semantic: CompiledBranchSemantic::AggregateNodes,
                    response_key,
                    field: "nodes".into(),
                    cardinality: Cardinality::Many,
                    nullable: false,
                    arguments: BTreeMap::new(),
                    dependencies: dependencies.to_vec(),
                    coverage,
                    filter,
                    order,
                    pagination,
                    relationship: None,
                    selection,
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

fn query_plan_field_dependencies(
    model: &ManifestModel,
    filter: Option<&ManifestFilterSemantics>,
    order: Option<&ManifestOrderSemantics>,
    declared_arguments: &[ManifestArgument],
    compiled_arguments: &BTreeMap<String, CompiledArgument>,
) -> BTreeSet<String> {
    let mut fields = BTreeSet::new();
    let mut relationships = BTreeSet::new();
    if let Some(filter) = filter {
        if let ManifestRowPolicy::Predicate { expression } = &filter.row_policy {
            collect_policy_fields(expression, &mut fields, &mut relationships);
        }
        if let Some(argument) = declared_arguments
            .iter()
            .find(|argument| argument.kind == ManifestArgumentKind::Filter)
            .and_then(|argument| compiled_arguments.get(&argument.name))
        {
            collect_filter_source_fields(argument, filter, &mut fields, &mut relationships);
        }
    }
    if let Some(order) = order {
        if let Some(argument) = declared_arguments
            .iter()
            .find(|argument| argument.kind == ManifestArgumentKind::Order)
            .and_then(|argument| compiled_arguments.get(&argument.name))
        {
            collect_order_source_fields(argument, order, &mut fields);
        }
    }
    for relationship_name in relationships {
        if let Some(relationship) = model.relationship(&relationship_name) {
            let (local, _) = relationship_key_fields(&relationship.key_mapping);
            fields.extend(local.iter().cloned());
        }
    }
    fields
}

fn collect_filter_source_fields(
    source: &CompiledArgument,
    semantics: &ManifestFilterSemantics,
    fields: &mut BTreeSet<String>,
    relationships: &mut BTreeSet<String>,
) {
    match source {
        CompiledArgument::Variable(_) => {
            fields.extend(semantics.fields.iter().map(|field| field.name.clone()));
            relationships.extend(semantics.relationships.iter().cloned());
        }
        CompiledArgument::Literal { value, .. } => {
            collect_client_filter_fields(value, semantics, fields, relationships);
        }
        CompiledArgument::List(items) => {
            for item in items {
                collect_filter_source_fields(item, semantics, fields, relationships);
            }
        }
        CompiledArgument::Object(values) => {
            for (name, value) in values {
                match name.as_str() {
                    "_and" | "_or" | "_not" => {
                        collect_filter_source_fields(value, semantics, fields, relationships);
                    }
                    field
                        if semantics
                            .fields
                            .iter()
                            .any(|candidate| candidate.name == field) =>
                    {
                        fields.insert(field.to_string());
                    }
                    relationship
                        if semantics
                            .relationships
                            .iter()
                            .any(|candidate| candidate == relationship) =>
                    {
                        relationships.insert(relationship.to_string());
                    }
                    _ => {}
                }
            }
        }
    }
}

fn collect_order_source_fields(
    source: &CompiledArgument,
    semantics: &ManifestOrderSemantics,
    fields: &mut BTreeSet<String>,
) {
    match source {
        CompiledArgument::Variable(_) => fields.extend(semantics.fields.iter().cloned()),
        CompiledArgument::Literal { value, .. } => {
            if let Some(entries) = value.as_array() {
                for entry in entries {
                    if let Some(object) = entry.as_object() {
                        fields.extend(object.keys().cloned());
                    }
                }
            }
        }
        CompiledArgument::List(items) => {
            for item in items {
                collect_order_source_fields(item, semantics, fields);
            }
        }
        CompiledArgument::Object(values) => fields.extend(values.keys().cloned()),
    }
}

fn collect_policy_fields(
    expression: &ManifestFilterExpr,
    fields: &mut BTreeSet<String>,
    relationships: &mut BTreeSet<String>,
) {
    match expression {
        ManifestFilterExpr::And(expressions) | ManifestFilterExpr::Or(expressions) => {
            for expression in expressions {
                collect_policy_fields(expression, fields, relationships);
            }
        }
        ManifestFilterExpr::Not(expression) => {
            collect_policy_fields(expression, fields, relationships)
        }
        ManifestFilterExpr::Cmp { column, .. }
        | ManifestFilterExpr::In { column, .. }
        | ManifestFilterExpr::IsNull { column, .. } => {
            fields.insert(column.clone());
        }
        ManifestFilterExpr::Rel { field, .. } => {
            relationships.insert(field.clone());
        }
    }
}

fn collect_client_filter_fields(
    value: &JsonValue,
    semantics: &ManifestFilterSemantics,
    fields: &mut BTreeSet<String>,
    relationships: &mut BTreeSet<String>,
) {
    let Some(object) = value.as_object() else {
        return;
    };
    for (name, value) in object {
        match name.as_str() {
            "_and" | "_or" => {
                if let Some(items) = value.as_array() {
                    for item in items {
                        collect_client_filter_fields(item, semantics, fields, relationships);
                    }
                }
            }
            "_not" => collect_client_filter_fields(value, semantics, fields, relationships),
            field
                if semantics
                    .fields
                    .iter()
                    .any(|candidate| candidate.name == field) =>
            {
                fields.insert(field.to_string());
            }
            relationship
                if semantics
                    .relationships
                    .iter()
                    .any(|candidate| candidate == relationship) =>
            {
                relationships.insert(relationship.to_string());
            }
            _ => {}
        }
    }
}

fn inject_dependency_fields(
    selection: &mut CompiledObject,
    model: &ManifestModel,
    dependencies: &BTreeSet<String>,
    document: &ClientDocument,
    position: Pos,
) -> Result<(), ClientCompileError> {
    let mut response_keys = selection
        .members
        .iter()
        .map(|member| match member {
            CompiledMember::Scalar(scalar) => scalar.response_key.clone(),
            CompiledMember::Branch(branch) => branch.response_key.clone(),
        })
        .collect::<BTreeSet<_>>();
    for dependency in dependencies {
        if selection.members.iter().any(|member| match member {
            CompiledMember::Scalar(scalar) => scalar.field == *dependency,
            CompiledMember::Branch(_) => false,
        }) {
            continue;
        }
        let field = model.field(dependency).ok_or_else(|| {
            source_error(
                "client.selection.dependency_denied",
                format!(
                    "query-index dependency `{dependency}` is not selectable on model `{}`",
                    model.id
                ),
                document,
                position,
            )
        })?;
        let response_key = allocate_wire_alias(dependency, &mut response_keys);
        selection
            .members
            .push(CompiledMember::Scalar(compiled_scalar(
                &response_key,
                field,
                false,
            )));
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

fn compiled_pagination_plan(coverage: &CompiledCoverage) -> CompiledPaginationPlan {
    match coverage.kind.as_str() {
        "complete" => CompiledPaginationPlan {
            kind: "complete".into(),
            insert: "local".into(),
            delete: "local".into(),
            reorder: "local".into(),
            stable_update: "local".into(),
        },
        "offset" => CompiledPaginationPlan {
            kind: "offset".into(),
            // Runtime locality is still fail-closed: these operations are
            // applied only when observed coverage proves a non-full first
            // page. Full, shifted, or ambiguous windows revalidate.
            insert: "local".into(),
            delete: "local".into(),
            reorder: "local".into(),
            stable_update: "local".into(),
        },
        other => CompiledPaginationPlan {
            kind: other.into(),
            insert: "revalidate".into(),
            delete: "revalidate".into(),
            reorder: "revalidate".into(),
            stable_update: "revalidate".into(),
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn compile_query_plans(
    model: &ManifestModel,
    filter: Option<&ManifestFilterSemantics>,
    order: Option<&ManifestOrderSemantics>,
    declared_arguments: &[ManifestArgument],
    compiled_arguments: &BTreeMap<String, CompiledArgument>,
    variables: &[CompiledVariable],
    manifest: &ClientManifest,
    document: &ClientDocument,
    field: &Field,
    pagination: Option<&CompiledCoverage>,
    filter_base_depth: u64,
    list_index: bool,
) -> Result<CompiledQueryPlans, ClientCompileError> {
    let filter_argument = declared_arguments
        .iter()
        .find(|argument| argument.kind == ManifestArgumentKind::Filter);
    let order_argument = declared_arguments
        .iter()
        .find(|argument| argument.kind == ManifestArgumentKind::Order);

    let filter_plan = match filter {
        Some(semantics) => {
            let mut variable_constraints = BTreeMap::new();
            let input = filter_argument
                .and_then(|argument| compiled_arguments.get(&argument.name))
                .cloned();
            if let Some(input) = &input {
                let input_type = filter_argument
                    .map(ManifestArgument::graphql_type)
                    .ok_or_else(|| {
                        ClientCompileError::manifest(
                            "client.manifest.filter_argument",
                            format!(
                                "model `{}` has filter semantics without an argument",
                                model.id
                            ),
                        )
                    })?;
                validate_filter_source(
                    input,
                    model,
                    &model.filter_input,
                    &input_type,
                    variables,
                    manifest,
                    &manifest.execution,
                    document,
                    argument_position(field, filter_argument.map(|value| value.name.as_str())),
                    filter_base_depth,
                    &mut variable_constraints,
                )?;
            }
            let fields = semantics
                .fields
                .iter()
                .map(|filter_field| {
                    let field = model.field(&filter_field.name).ok_or_else(|| {
                        ClientCompileError::manifest(
                            "client.manifest.filter_field",
                            format!(
                                "filter plan for model `{}` references absent field `{}`",
                                model.id, filter_field.name
                            ),
                        )
                    })?;
                    Ok(CompiledFilterField {
                        name: field.name.clone(),
                        scalar: field.scalar.clone(),
                        codec: field.codec.clone(),
                        nullable: field.nullable,
                        operators: filter_field.operators.clone(),
                    })
                })
                .collect::<Result<Vec<_>, ClientCompileError>>()?;
            let relationships = semantics
                .relationships
                .iter()
                .map(|name| {
                    model
                        .relationship(name)
                        .map(compiled_relationship_plan)
                        .ok_or_else(|| {
                            ClientCompileError::manifest(
                                "client.manifest.filter_relationship",
                                format!(
                                    "filter plan for model `{}` references absent relationship `{name}`",
                                    model.id
                                ),
                            )
                        })
                })
                .collect::<Result<Vec<_>, ClientCompileError>>()?;
            Some(CompiledFilterPlan {
                input,
                fields,
                relationships,
                row_policy: semantics.row_policy.clone(),
                variable_constraints,
            })
        }
        None => None,
    };

    let order_plan = match order {
        Some(semantics) => {
            let input = order_argument
                .and_then(|argument| compiled_arguments.get(&argument.name))
                .cloned();
            if let Some(input) = &input {
                validate_order_source(
                    input,
                    semantics,
                    order_argument,
                    variables,
                    document,
                    argument_position(field, order_argument.map(|value| value.name.as_str())),
                )?;
            }
            let fields = semantics
                .fields
                .iter()
                .map(|name| compiled_order_field(model, name))
                .collect::<Result<Vec<_>, _>>()?;
            let identity = model
                .identity()
                .unwrap_or_default()
                .iter()
                .map(|identity| compiled_order_field(model, &identity.name))
                .collect::<Result<Vec<_>, _>>()?;
            Some(CompiledOrderPlan {
                input,
                fields,
                identity,
            })
        }
        None if list_index => Some(CompiledOrderPlan {
            input: None,
            fields: Vec::new(),
            identity: model
                .identity()
                .unwrap_or_default()
                .iter()
                .map(|identity| compiled_order_field(model, &identity.name))
                .collect::<Result<Vec<_>, _>>()?,
        }),
        None => None,
    };

    let pagination_plan = if list_index {
        pagination.map(compiled_pagination_plan)
    } else {
        None
    };

    Ok((filter_plan, order_plan, pagination_plan))
}

fn compiled_order_field(
    model: &ManifestModel,
    name: &str,
) -> Result<CompiledOrderField, ClientCompileError> {
    let field = model.field(name).ok_or_else(|| {
        ClientCompileError::manifest(
            "client.manifest.order_field",
            format!(
                "order plan for model `{}` references absent field `{name}`",
                model.id
            ),
        )
    })?;
    Ok(CompiledOrderField {
        name: field.name.clone(),
        scalar: field.scalar.clone(),
        codec: field.codec.clone(),
        nullable: field.nullable,
    })
}

fn argument_position(field: &Field, name: Option<&str>) -> Pos {
    name.and_then(|name| {
        field
            .arguments
            .iter()
            .find(|(argument, _)| argument.node.as_str() == name)
            .map(|(_, value)| value.pos)
    })
    .unwrap_or(field.name.pos)
}

fn filter_depth_from_selection(selection_depth: usize, root_offset: usize) -> u64 {
    u64::try_from(selection_depth.saturating_sub(root_offset))
        .expect("compiler selection depth fits in u64")
}

#[allow(clippy::too_many_arguments)]
fn validate_filter_source(
    source: &CompiledArgument,
    model: &ManifestModel,
    input: &ManifestFilterInput,
    expected_type: &str,
    variables: &[CompiledVariable],
    manifest: &ClientManifest,
    execution: &ManifestExecutionLimits,
    document: &ClientDocument,
    position: Pos,
    depth: u64,
    constraints: &mut BTreeMap<String, VariableUseConstraint>,
) -> Result<(), ClientCompileError> {
    validate_filter_depth(model, execution, depth, document, position)?;
    match source {
        CompiledArgument::Literal { value, .. } => validate_filter_literal(
            value, model, input, manifest, execution, document, position, depth,
        ),
        CompiledArgument::Variable(name) => {
            validate_nested_variable(name, expected_type, variables, document, position)?;
            constrain_variable(constraints, name, VariableUseConstraint::filter(depth));
            Ok(())
        }
        CompiledArgument::List(_) => Err(source_error(
            "client.filter.object_required",
            format!("filter for model `{}` must be an object or null", model.id),
            document,
            position,
        )),
        CompiledArgument::Object(values) => {
            for (name, value) in values {
                match name.as_str() {
                    "_and" | "_or" => match value {
                        CompiledArgument::List(items) => {
                            validate_filter_width(
                                name,
                                items.len(),
                                execution.max_bool_width,
                                document,
                                position,
                            )?;
                            for item in items {
                                if let CompiledArgument::Variable(variable) = item {
                                    validate_nested_variable(
                                        variable,
                                        &format!("{}!", input.type_name),
                                        variables,
                                        document,
                                        position,
                                    )?;
                                    constrain_variable(
                                        constraints,
                                        variable,
                                        VariableUseConstraint::filter(depth.saturating_add(1)),
                                    );
                                } else {
                                    validate_filter_source(
                                        item,
                                        model,
                                        input,
                                        &input.type_name,
                                        variables,
                                        manifest,
                                        execution,
                                        document,
                                        position,
                                        depth.saturating_add(1),
                                        constraints,
                                    )?;
                                }
                            }
                        }
                        CompiledArgument::Variable(variable) => {
                            validate_nested_variable(
                                variable,
                                &format!("[{}!]", input.type_name),
                                variables,
                                document,
                                position,
                            )?;
                            constrain_variable(
                                constraints,
                                variable,
                                VariableUseConstraint::list(
                                    execution.max_bool_width,
                                    Some(VariableUseConstraint::filter(depth.saturating_add(1))),
                                ),
                            );
                        }
                        CompiledArgument::Literal { value, .. } => {
                            validate_filter_literal(
                                &json_singleton(name, value.clone()),
                                model,
                                input,
                                manifest,
                                execution,
                                document,
                                position,
                                depth,
                            )?;
                        }
                        CompiledArgument::Object(_) => {
                            return Err(source_error(
                                "client.filter.boolean_list",
                                format!("filter operator `{name}` must contain a list of objects"),
                                document,
                                position,
                            ));
                        }
                    },
                    "_not" => validate_filter_source(
                        value,
                        model,
                        input,
                        &input.type_name,
                        variables,
                        manifest,
                        execution,
                        document,
                        position,
                        depth.saturating_add(1),
                        constraints,
                    )?,
                    field_name => {
                        if let Some(filter_field) =
                            input.fields.iter().find(|field| field.name == field_name)
                        {
                            let field = model.field(field_name).ok_or_else(|| {
                                ClientCompileError::manifest(
                                    "client.manifest.filter_field",
                                    format!(
                                        "filter plan for model `{}` references absent field `{field_name}`",
                                        model.id
                                    ),
                                )
                            })?;
                            validate_filter_comparison_source(
                                value,
                                model,
                                field,
                                filter_field,
                                variables,
                                execution,
                                document,
                                position,
                                constraints,
                            )?;
                        } else if let Some(relationship_input) = input
                            .relationships
                            .iter()
                            .find(|relationship| relationship.field == field_name)
                        {
                            let relationship = model.relationship(field_name).ok_or_else(|| {
                                ClientCompileError::manifest(
                                    "client.manifest.filter_relationship",
                                    format!(
                                        "filter plan for model `{}` references absent relationship `{field_name}`",
                                        model.id
                                    ),
                                )
                            })?;
                            let target = manifest.models.get(&relationship.target_model).ok_or_else(|| {
                                ClientCompileError::manifest(
                                    "client.manifest.filter_relationship",
                                    format!(
                                        "filter relationship `{}.{field_name}` has an absent target",
                                        model.id
                                    ),
                                )
                            })?;
                            validate_filter_source(
                                value,
                                target,
                                &target.filter_input,
                                &relationship_input.target_type,
                                variables,
                                manifest,
                                execution,
                                document,
                                position,
                                depth.saturating_add(1),
                                constraints,
                            )?;
                        } else {
                            return Err(source_error(
                                "client.filter.field_denied_or_unknown",
                                format!(
                                    "filter field `{field_name}` is absent from selected model `{}`",
                                    model.id
                                ),
                                document,
                                position,
                            ));
                        }
                    }
                }
            }
            Ok(())
        }
    }
}

fn validate_filter_depth(
    model: &ManifestModel,
    execution: &ManifestExecutionLimits,
    depth: u64,
    document: &ClientDocument,
    position: Pos,
) -> Result<(), ClientCompileError> {
    if depth > MAX_OBJECT_DEPTH as u64 {
        return Err(source_error(
            "client.filter.safety_depth",
            format!(
                "filter for model `{}` exceeds the compiler safety depth {MAX_OBJECT_DEPTH}",
                model.id
            ),
            document,
            position,
        ));
    }
    if depth > execution.max_depth {
        return Err(source_error(
            "client.filter.depth_limit",
            format!(
                "filter for model `{}` reaches semantic depth {depth}, exceeding max_depth {}",
                model.id, execution.max_depth
            ),
            document,
            position,
        ));
    }
    Ok(())
}

fn validate_filter_width(
    operator: &str,
    actual: usize,
    maximum: u64,
    document: &ClientDocument,
    position: Pos,
) -> Result<(), ClientCompileError> {
    let actual = u64::try_from(actual).unwrap_or(u64::MAX);
    if actual <= maximum {
        return Ok(());
    }
    Err(source_error(
        "client.filter.width_limit",
        format!(
            "filter operator `{operator}` contains {actual} items, exceeding its limit {maximum}"
        ),
        document,
        position,
    ))
}

fn constrain_variable(
    constraints: &mut BTreeMap<String, VariableUseConstraint>,
    name: &str,
    constraint: VariableUseConstraint,
) {
    constraints
        .entry(name.to_string())
        .and_modify(|existing| existing.intersect(&constraint))
        .or_insert(constraint);
}

#[allow(clippy::too_many_arguments)]
fn validate_filter_comparison_source(
    source: &CompiledArgument,
    model: &ManifestModel,
    field: &ManifestField,
    semantics: &super::manifest::ManifestFilterField,
    variables: &[CompiledVariable],
    execution: &ManifestExecutionLimits,
    document: &ClientDocument,
    position: Pos,
    constraints: &mut BTreeMap<String, VariableUseConstraint>,
) -> Result<(), ClientCompileError> {
    if let CompiledArgument::Literal { value, .. } = source {
        let operators = value.as_object().ok_or_else(|| {
            source_error(
                "client.filter.comparison_object",
                format!(
                    "filter field `{}.{}` must contain a comparison object",
                    model.id, field.name
                ),
                document,
                position,
            )
        })?;
        for (operator, operand) in operators {
            if !semantics
                .operators
                .iter()
                .any(|allowed| allowed == operator)
            {
                return Err(source_error(
                    "client.filter.operator_denied_or_unknown",
                    format!(
                        "filter operator `{operator}` is absent from selected field `{}.{}`",
                        model.id, field.name
                    ),
                    document,
                    position,
                ));
            }
            match operator.as_str() {
                "_in" | "_nin" => {
                    let items = operand.as_array().ok_or_else(|| {
                        source_error(
                            "client.filter.list_required",
                            format!("filter operator `{operator}` requires a list"),
                            document,
                            position,
                        )
                    })?;
                    validate_filter_width(
                        operator,
                        items.len(),
                        execution.max_in_list,
                        document,
                        position,
                    )?;
                    for item in items {
                        validate_filter_scalar_literal(
                            item, model, field, false, document, position,
                        )?;
                    }
                }
                "_is_null" if !operand.is_boolean() && !operand.is_null() => {
                    return Err(source_error(
                        "client.filter.boolean_required",
                        "filter operator `_is_null` requires a boolean or null",
                        document,
                        position,
                    ));
                }
                "_is_null" => {}
                "_has_key" => validate_filter_typed_literal(
                    operand, model, field, "String", "string", true, document, position,
                )?,
                _ => {
                    validate_filter_scalar_literal(operand, model, field, true, document, position)?
                }
            }
        }
        return Ok(());
    }
    let CompiledArgument::Object(operators) = source else {
        return Err(source_error(
            "client.filter.comparison_object",
            format!(
                "filter field `{}.{}` must contain a comparison object",
                model.id, field.name
            ),
            document,
            position,
        ));
    };
    for (operator, operand) in operators {
        if !semantics
            .operators
            .iter()
            .any(|allowed| allowed == operator)
        {
            return Err(source_error(
                "client.filter.operator_denied_or_unknown",
                format!(
                    "filter operator `{operator}` is absent from selected field `{}.{}`",
                    model.id, field.name
                ),
                document,
                position,
            ));
        }
        match (operator.as_str(), operand) {
            ("_in" | "_nin", CompiledArgument::Variable(variable)) => {
                validate_nested_variable(
                    variable,
                    &format!("[{}!]", field.scalar),
                    variables,
                    document,
                    position,
                )?;
                constrain_variable(
                    constraints,
                    variable,
                    VariableUseConstraint::list(execution.max_in_list, None),
                );
            }
            ("_in" | "_nin", CompiledArgument::List(items)) => {
                validate_filter_width(
                    operator,
                    items.len(),
                    execution.max_in_list,
                    document,
                    position,
                )?;
                for item in items {
                    match item {
                        CompiledArgument::Variable(variable) => validate_nested_variable(
                            variable,
                            &format!("{}!", field.scalar),
                            variables,
                            document,
                            position,
                        )?,
                        item => validate_compiled_filter_literal(
                            item, model, field, false, document, position,
                        )?,
                    }
                }
            }
            (
                "_in" | "_nin",
                CompiledArgument::Literal {
                    value: JsonValue::Array(items),
                    ..
                },
            ) => {
                validate_filter_width(
                    operator,
                    items.len(),
                    execution.max_in_list,
                    document,
                    position,
                )?;
                for item in items {
                    validate_filter_scalar_literal(item, model, field, false, document, position)?;
                }
            }
            ("_in" | "_nin", _) => {
                return Err(source_error(
                    "client.filter.list_required",
                    format!("filter operator `{operator}` requires a list"),
                    document,
                    position,
                ));
            }
            ("_is_null", CompiledArgument::Variable(variable)) => {
                validate_nested_variable(variable, "Boolean", variables, document, position)?;
            }
            (
                "_is_null",
                CompiledArgument::Literal {
                    value: JsonValue::Bool(_) | JsonValue::Null,
                    ..
                },
            ) => {}
            ("_is_null", _) => {
                return Err(source_error(
                    "client.filter.boolean_required",
                    "filter operator `_is_null` requires a boolean or null",
                    document,
                    position,
                ));
            }
            ("_has_key", CompiledArgument::Variable(variable)) => {
                validate_nested_variable(variable, "String", variables, document, position)?;
            }
            ("_has_key", operand) => validate_compiled_filter_typed_literal(
                operand, model, field, "String", "string", true, document, position,
            )?,
            (_, CompiledArgument::Variable(variable)) => {
                validate_nested_variable(variable, &field.scalar, variables, document, position)?;
            }
            (_, operand) => {
                validate_compiled_filter_literal(operand, model, field, true, document, position)?
            }
        }
    }
    Ok(())
}

fn validate_compiled_filter_literal(
    source: &CompiledArgument,
    model: &ManifestModel,
    field: &ManifestField,
    nullable: bool,
    document: &ClientDocument,
    position: Pos,
) -> Result<(), ClientCompileError> {
    validate_compiled_filter_typed_literal(
        source,
        model,
        field,
        &field.scalar,
        &field.codec,
        nullable,
        document,
        position,
    )
}

#[allow(clippy::too_many_arguments)]
fn validate_compiled_filter_typed_literal(
    source: &CompiledArgument,
    model: &ManifestModel,
    field: &ManifestField,
    scalar: &str,
    codec: &str,
    nullable: bool,
    document: &ClientDocument,
    position: Pos,
) -> Result<(), ClientCompileError> {
    let Some(value) = compiled_literal_json(source) else {
        return Err(source_error(
            "client.filter.operand",
            format!(
                "filter operand on `{}.{}` cannot contain a nested variable",
                model.id, field.name
            ),
            document,
            position,
        ));
    };
    validate_filter_typed_literal(
        &value, model, field, scalar, codec, nullable, document, position,
    )
}

fn compiled_literal_json(source: &CompiledArgument) -> Option<JsonValue> {
    match source {
        CompiledArgument::Literal { value, .. } => Some(value.clone()),
        CompiledArgument::Variable(_) => None,
        CompiledArgument::List(items) => items
            .iter()
            .map(compiled_literal_json)
            .collect::<Option<Vec<_>>>()
            .map(JsonValue::Array),
        CompiledArgument::Object(fields) => fields
            .iter()
            .map(|(name, value)| Some((name.clone(), compiled_literal_json(value)?)))
            .collect::<Option<JsonMap<String, JsonValue>>>()
            .map(JsonValue::Object),
    }
}

fn validate_filter_scalar_literal(
    value: &JsonValue,
    model: &ManifestModel,
    field: &ManifestField,
    nullable: bool,
    document: &ClientDocument,
    position: Pos,
) -> Result<(), ClientCompileError> {
    validate_filter_typed_literal(
        value,
        model,
        field,
        &field.scalar,
        &field.codec,
        nullable,
        document,
        position,
    )
}

#[allow(clippy::too_many_arguments)]
fn validate_filter_typed_literal(
    value: &JsonValue,
    model: &ManifestModel,
    field: &ManifestField,
    scalar: &str,
    codec: &str,
    nullable: bool,
    document: &ClientDocument,
    position: Pos,
) -> Result<(), ClientCompileError> {
    if scalar_json_literal_matches(value, scalar, codec, nullable) {
        return Ok(());
    }
    Err(source_error(
        "client.filter.literal_type",
        format!(
            "filter literal for `{}.{}` does not match scalar `{}` / codec `{}`{}",
            model.id,
            field.name,
            scalar,
            codec,
            if nullable { " or null" } else { "" }
        ),
        document,
        position,
    ))
}

fn scalar_json_literal_matches(
    value: &JsonValue,
    scalar: &str,
    codec: &str,
    nullable: bool,
) -> bool {
    match (scalar, codec, value) {
        (_, _, JsonValue::Null) => nullable,
        ("ID" | "String", "string", JsonValue::String(_))
        | ("Timestamptz", "string_unvalidated_timestamp", JsonValue::String(_))
        | ("Boolean", "boolean", JsonValue::Bool(_)) => true,
        ("Bytea", "base64", JsonValue::String(value)) => canonical_standard_base64(value).is_some(),
        ("Int", "int32", JsonValue::Number(number)) => {
            number
                .as_i64()
                .is_some_and(|number| i32::try_from(number).is_ok())
                || number
                    .as_u64()
                    .is_some_and(|number| i32::try_from(number).is_ok())
        }
        ("Float", "float64", JsonValue::Number(number)) => {
            number.as_f64().is_some_and(f64::is_finite)
        }
        ("BigInt", "json_number_precision_limited", JsonValue::Number(number)) => {
            json_number_is_safe_integer(number)
        }
        ("JSON", "json", value) => json_value_roundtrips_javascript(value),
        _ => false,
    }
}

fn json_value_roundtrips_javascript(value: &JsonValue) -> bool {
    match value {
        JsonValue::Number(number) => json_number_roundtrips_javascript(number),
        JsonValue::Array(values) => values.iter().all(json_value_roundtrips_javascript),
        JsonValue::Object(values) => values.values().all(json_value_roundtrips_javascript),
        JsonValue::Null | JsonValue::Bool(_) | JsonValue::String(_) => true,
    }
}

fn json_number_roundtrips_javascript(number: &serde_json::Number) -> bool {
    let Some(value) = number.as_f64().filter(|value| value.is_finite()) else {
        return false;
    };
    value.fract() != 0.0 || value.abs() <= 9_007_199_254_740_991.0
}

fn json_number_is_safe_integer(number: &serde_json::Number) -> bool {
    const JS_MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
    json_number_is_negative_zero(number)
        || number
            .as_i64()
            .is_some_and(|number| number.unsigned_abs() <= JS_MAX_SAFE_INTEGER)
        || number
            .as_u64()
            .is_some_and(|number| number <= JS_MAX_SAFE_INTEGER)
}

fn json_number_is_negative_zero(number: &serde_json::Number) -> bool {
    number
        .as_f64()
        .is_some_and(|number| number == 0.0 && number.is_sign_negative())
}

fn canonical_standard_base64(value: &str) -> Option<String> {
    base64::engine::general_purpose::STANDARD
        .decode(value.as_bytes())
        .ok()
        .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes))
}

fn validate_nested_variable(
    name: &str,
    expected: &str,
    variables: &[CompiledVariable],
    document: &ClientDocument,
    position: Pos,
) -> Result<(), ClientCompileError> {
    let variable = variables
        .iter()
        .find(|variable| variable.name == name)
        .expect("nested variable existence checked while compiling arguments");
    let expected_type = Type::new(expected).ok_or_else(|| {
        ClientCompileError::manifest(
            "client.manifest.argument_type",
            format!("invalid nested variable target type `{expected}`"),
        )
    })?;
    if variable_type_compatible(&variable.graphql_type, &expected_type) {
        return Ok(());
    }
    Err(source_error(
        "client.variable.type_mismatch",
        format!(
            "nested variable `${name}` has type `{}`; selected filter/order position requires `{expected}`",
            variable.graphql_type
        ),
        document,
        position,
    ))
}

fn json_singleton(name: &str, value: JsonValue) -> JsonValue {
    let mut object = JsonMap::new();
    object.insert(name.to_string(), value);
    JsonValue::Object(object)
}

#[allow(clippy::too_many_arguments)]
fn validate_filter_literal(
    value: &JsonValue,
    model: &ManifestModel,
    input: &ManifestFilterInput,
    manifest: &ClientManifest,
    execution: &ManifestExecutionLimits,
    document: &ClientDocument,
    position: Pos,
    depth: u64,
) -> Result<(), ClientCompileError> {
    validate_filter_depth(model, execution, depth, document, position)?;
    if value.is_null() {
        return Ok(());
    }
    let object = value.as_object().ok_or_else(|| {
        source_error(
            "client.filter.object_required",
            format!("filter for model `{}` must be an object or null", model.id),
            document,
            position,
        )
    })?;
    for (name, value) in object {
        match name.as_str() {
            "_and" | "_or" => {
                if value.is_null() {
                    continue;
                }
                let items = value.as_array().ok_or_else(|| {
                    source_error(
                        "client.filter.boolean_list",
                        format!("filter operator `{name}` must contain a list of objects"),
                        document,
                        position,
                    )
                })?;
                validate_filter_width(
                    name,
                    items.len(),
                    execution.max_bool_width,
                    document,
                    position,
                )?;
                for item in items {
                    validate_filter_literal(
                        item,
                        model,
                        input,
                        manifest,
                        execution,
                        document,
                        position,
                        depth.saturating_add(1),
                    )?;
                }
            }
            "_not" => validate_filter_literal(
                value,
                model,
                input,
                manifest,
                execution,
                document,
                position,
                depth.saturating_add(1),
            )?,
            field_name => {
                if let Some(field) = input.fields.iter().find(|field| field.name == field_name) {
                    let operators = value.as_object().ok_or_else(|| {
                        source_error(
                            "client.filter.comparison_object",
                            format!(
                                "filter field `{}.{field_name}` must contain a comparison object",
                                model.id
                            ),
                            document,
                            position,
                        )
                    })?;
                    for (operator, operand) in operators {
                        if !field.operators.iter().any(|allowed| allowed == operator) {
                            return Err(source_error(
                                "client.filter.operator_denied_or_unknown",
                                format!(
                                    "filter operator `{operator}` is absent from selected field `{}.{field_name}`",
                                    model.id
                                ),
                                document,
                                position,
                            ));
                        }
                        let model_field = model.field(field_name).ok_or_else(|| {
                            ClientCompileError::manifest(
                                "client.manifest.filter_field",
                                format!(
                                    "filter plan for model `{}` references absent field `{field_name}`",
                                    model.id
                                ),
                            )
                        })?;
                        match operator.as_str() {
                            "_in" | "_nin" => {
                                let items = operand.as_array().ok_or_else(|| {
                                    source_error(
                                        "client.filter.list_required",
                                        format!("filter operator `{operator}` requires a list"),
                                        document,
                                        position,
                                    )
                                })?;
                                validate_filter_width(
                                    operator,
                                    items.len(),
                                    execution.max_in_list,
                                    document,
                                    position,
                                )?;
                                for item in items {
                                    validate_filter_scalar_literal(
                                        item,
                                        model,
                                        model_field,
                                        false,
                                        document,
                                        position,
                                    )?;
                                }
                            }
                            "_is_null" if !operand.is_boolean() && !operand.is_null() => {
                                return Err(source_error(
                                    "client.filter.boolean_required",
                                    "filter operator `_is_null` requires a boolean or null",
                                    document,
                                    position,
                                ));
                            }
                            "_is_null" => {}
                            _ => validate_filter_scalar_literal(
                                operand,
                                model,
                                model_field,
                                true,
                                document,
                                position,
                            )?,
                        }
                    }
                } else if let Some(relationship_input) = input
                    .relationships
                    .iter()
                    .find(|relationship| relationship.field == field_name)
                {
                    let relationship = model.relationship(field_name).ok_or_else(|| {
                        ClientCompileError::manifest(
                            "client.manifest.filter_relationship",
                            format!(
                                "filter plan for model `{}` references absent relationship `{field_name}`",
                                model.id
                            ),
                        )
                    })?;
                    let target =
                        manifest
                            .models
                            .get(&relationship.target_model)
                            .ok_or_else(|| {
                                ClientCompileError::manifest(
                                    "client.manifest.filter_relationship",
                                    format!(
                                "filter relationship `{}.{field_name}` has an absent target",
                                model.id
                            ),
                                )
                            })?;
                    if relationship_input.target_type != target.filter_input.type_name {
                        return Err(ClientCompileError::manifest(
                            "client.manifest.filter_relationship",
                            format!(
                                "filter relationship `{}.{field_name}` targets input `{}` but model `{}` declares `{}`",
                                model.id,
                                relationship_input.target_type,
                                target.id,
                                target.filter_input.type_name
                            ),
                        ));
                    }
                    validate_filter_literal(
                        value,
                        target,
                        &target.filter_input,
                        manifest,
                        execution,
                        document,
                        position,
                        depth.saturating_add(1),
                    )?;
                } else {
                    return Err(source_error(
                        "client.filter.field_denied_or_unknown",
                        format!(
                            "filter field `{field_name}` is absent from selected model `{}`",
                            model.id
                        ),
                        document,
                        position,
                    ));
                }
            }
        }
    }
    Ok(())
}

fn validate_order_literal(
    value: &JsonValue,
    semantics: &ManifestOrderSemantics,
    document: &ClientDocument,
    position: Pos,
) -> Result<(), ClientCompileError> {
    if value.is_null() {
        return Ok(());
    }
    let entries = value.as_array().ok_or_else(|| {
        source_error(
            "client.order.list_required",
            "order_by must be a list or null",
            document,
            position,
        )
    })?;
    for entry in entries {
        let object = entry.as_object().ok_or_else(|| {
            source_error(
                "client.order.object_required",
                "each order_by entry must be an object",
                document,
                position,
            )
        })?;
        if object.len() != 1 {
            return Err(source_error(
                "client.order.ambiguous",
                "each order_by entry must contain exactly one field to declare priority",
                document,
                position,
            ));
        }
        let (field, direction) = object.iter().next().expect("length checked");
        if !semantics.fields.iter().any(|allowed| allowed == field) {
            return Err(source_error(
                "client.order.field_denied_or_unknown",
                format!("order_by field `{field}` is absent from the selected model"),
                document,
                position,
            ));
        }
        let direction = direction.as_str().ok_or_else(|| {
            source_error(
                "client.order.direction",
                format!("order_by field `{field}` must use a declared direction enum"),
                document,
                position,
            )
        })?;
        if !semantics.values.iter().any(|allowed| allowed == direction) {
            return Err(source_error(
                "client.order.direction",
                format!("order_by direction `{direction}` is absent from the selected manifest"),
                document,
                position,
            ));
        }
    }
    Ok(())
}

fn validate_order_source(
    source: &CompiledArgument,
    semantics: &ManifestOrderSemantics,
    argument: Option<&ManifestArgument>,
    variables: &[CompiledVariable],
    document: &ClientDocument,
    position: Pos,
) -> Result<(), ClientCompileError> {
    match source {
        CompiledArgument::Literal { value, .. } => {
            validate_order_literal(value, semantics, document, position)
        }
        CompiledArgument::Variable(name) => {
            let expected = argument
                .map(ManifestArgument::graphql_type)
                .ok_or_else(|| {
                    ClientCompileError::manifest(
                        "client.manifest.order_argument",
                        "order semantics exist without an order argument",
                    )
                })?;
            validate_nested_variable(name, &expected, variables, document, position)
        }
        CompiledArgument::List(entries) => {
            for entry in entries {
                match entry {
                    CompiledArgument::Variable(name) => {
                        let item_type = argument
                            .map(|argument| argument.type_name.as_str())
                            .ok_or_else(|| {
                                ClientCompileError::manifest(
                                    "client.manifest.order_argument",
                                    "order semantics exist without an order argument",
                                )
                            })?;
                        validate_nested_variable(
                            name,
                            &format!("{item_type}!"),
                            variables,
                            document,
                            position,
                        )?;
                    }
                    CompiledArgument::Object(fields) => validate_order_entry_source(
                        fields, semantics, variables, document, position,
                    )?,
                    CompiledArgument::Literal { value, .. } => {
                        validate_order_literal(
                            &JsonValue::Array(vec![value.clone()]),
                            semantics,
                            document,
                            position,
                        )?;
                    }
                    CompiledArgument::List(_) => {
                        return Err(source_error(
                            "client.order.object_required",
                            "each order_by entry must be an object",
                            document,
                            position,
                        ));
                    }
                }
            }
            Ok(())
        }
        CompiledArgument::Object(_) => Err(source_error(
            "client.order.list_required",
            "order_by must be a list or null",
            document,
            position,
        )),
    }
}

fn validate_order_entry_source(
    fields: &BTreeMap<String, CompiledArgument>,
    semantics: &ManifestOrderSemantics,
    variables: &[CompiledVariable],
    document: &ClientDocument,
    position: Pos,
) -> Result<(), ClientCompileError> {
    if fields.len() != 1 {
        return Err(source_error(
            "client.order.ambiguous",
            "each order_by entry must contain exactly one field to declare priority",
            document,
            position,
        ));
    }
    let (field, direction) = fields.iter().next().expect("length checked");
    if !semantics.fields.iter().any(|allowed| allowed == field) {
        return Err(source_error(
            "client.order.field_denied_or_unknown",
            format!("order_by field `{field}` is absent from the selected model"),
            document,
            position,
        ));
    }
    match direction {
        CompiledArgument::Variable(name) => {
            validate_nested_variable(name, "order_by", variables, document, position)
        }
        CompiledArgument::Literal { value, .. } => {
            let direction = value.as_str().ok_or_else(|| {
                source_error(
                    "client.order.direction",
                    format!("order_by field `{field}` must use a declared direction enum"),
                    document,
                    position,
                )
            })?;
            if semantics.values.iter().any(|allowed| allowed == direction) {
                Ok(())
            } else {
                Err(source_error(
                    "client.order.direction",
                    format!(
                        "order_by direction `{direction}` is absent from the selected manifest"
                    ),
                    document,
                    position,
                ))
            }
        }
        CompiledArgument::List(_) | CompiledArgument::Object(_) => Err(source_error(
            "client.order.direction",
            format!("order_by field `{field}` must use a declared direction enum"),
            document,
            position,
        )),
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
                "`@load` operation `{operation}` is outside `src/routes/**/+page.graphql` or `+page.query.ts`; move it there or register `--route {operation}=/route-id`"
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
    let directory = page_document_directory(rest)?;
    if directory.is_empty() {
        Some("/".into())
    } else {
        Some(format!("/{directory}"))
    }
}

/// Conventional SSR document names colocated with SvelteKit routes.
///
/// `+page.graphql` is authored GraphQL. `+page.query.ts` is a TypeScript
/// `defineQuery` module whose toolchain-materialized body is still GraphQL
/// document text (the path is provenance only).
fn page_document_directory(rest: &str) -> Option<&str> {
    const SUFFIXES: &[&str] = &["+page.graphql", "+page.query.ts"];
    for suffix in SUFFIXES {
        if rest == *suffix {
            return Some("");
        }
        if let Some(directory) = rest.strip_suffix(&format!("/{suffix}")) {
            return Some(directory);
        }
    }
    None
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
            .map(|(name, value)| format!("{name}: {}", render_compiled_argument(value)))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn render_compiled_argument(value: &CompiledArgument) -> String {
    match value {
        CompiledArgument::Literal { wire, .. } => wire.clone(),
        CompiledArgument::Variable(variable) => format!("${variable}"),
        CompiledArgument::List(values) => format!(
            "[{}]",
            values
                .iter()
                .map(render_compiled_argument)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        CompiledArgument::Object(values) => format!(
            "{{{}}}",
            values
                .iter()
                .map(|(name, value)| format!("{name}: {}", render_compiled_argument(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
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

fn compile_argument_source(
    value: &Value,
    argument: &str,
    variables: &BTreeMap<&str, &CompiledVariable>,
    used_variables: &mut BTreeSet<String>,
    document: &ClientDocument,
    position: Pos,
) -> Result<CompiledArgument, ClientCompileError> {
    if !contains_variable(value) {
        return Ok(CompiledArgument::Literal {
            value: value_to_json(value, document, position)?,
            wire: render_value(value, document, position)?,
        });
    }
    match value {
        Value::Variable(variable) => {
            if !variables.contains_key(variable.as_str()) {
                return Err(source_error(
                    "client.variable.undefined",
                    format!(
                        "argument `{argument}` references undefined nested variable `${variable}`"
                    ),
                    document,
                    position,
                ));
            }
            used_variables.insert(variable.to_string());
            Ok(CompiledArgument::Variable(variable.to_string()))
        }
        Value::List(values) => values
            .iter()
            .map(|value| {
                compile_argument_source(
                    value,
                    argument,
                    variables,
                    used_variables,
                    document,
                    position,
                )
            })
            .collect::<Result<Vec<_>, _>>()
            .map(CompiledArgument::List),
        Value::Object(values) => values
            .iter()
            .map(|(name, value)| {
                Ok((
                    name.to_string(),
                    compile_argument_source(
                        value,
                        argument,
                        variables,
                        used_variables,
                        document,
                        position,
                    )?,
                ))
            })
            .collect::<Result<BTreeMap<_, _>, ClientCompileError>>()
            .map(CompiledArgument::Object),
        _ => unreachable!("non-container value with no variable returned above"),
    }
}

fn contains_variable(value: &Value) -> bool {
    match value {
        Value::Variable(_) => true,
        Value::List(values) => values.iter().any(contains_variable),
        Value::Object(values) => values.values().any(contains_variable),
        _ => false,
    }
}

fn canonicalize_argument_literal(
    value: &Value,
    argument: &ManifestArgument,
    model: &ManifestModel,
    manifest: &ClientManifest,
) -> Value {
    let value = if argument.kind == ManifestArgumentKind::Filter {
        canonicalize_filter_literal(value, model, &model.filter_input, manifest)
    } else {
        value.clone()
    };
    let value =
        if argument.list && !matches!(&value, Value::List(_) | Value::Null | Value::Variable(_)) {
            Value::List(vec![value])
        } else {
            value
        };
    match &argument.codec {
        Some(codec) if argument.list => match value {
            Value::List(items) => Value::List(
                items
                    .iter()
                    .map(|item| canonicalize_scalar_literal(item, &argument.type_name, codec))
                    .collect(),
            ),
            value => value,
        },
        Some(codec) => canonicalize_scalar_literal(&value, &argument.type_name, codec),
        None => value,
    }
}

fn canonicalize_filter_literal(
    value: &Value,
    model: &ManifestModel,
    input: &ManifestFilterInput,
    manifest: &ClientManifest,
) -> Value {
    let Value::Object(fields) = value else {
        return value.clone();
    };
    let mut canonical = fields.clone();
    for (name, value) in fields {
        let value = match name.as_str() {
            "_and" | "_or" => canonicalize_filter_literal_list(value, model, input, manifest),
            "_not" => canonicalize_filter_literal(value, model, input, manifest),
            field_name if input.fields.iter().any(|field| field.name == field_name) => model
                .field(field_name)
                .map(|field| canonicalize_filter_comparison_literal(value, field))
                .unwrap_or_else(|| value.clone()),
            relationship_name
                if input
                    .relationships
                    .iter()
                    .any(|relationship| relationship.field == relationship_name) =>
            {
                model
                    .relationship(relationship_name)
                    .and_then(|relationship| manifest.models.get(&relationship.target_model))
                    .map(|target| {
                        canonicalize_filter_literal(value, target, &target.filter_input, manifest)
                    })
                    .unwrap_or_else(|| value.clone())
            }
            _ => value.clone(),
        };
        canonical.insert(name.clone(), value);
    }
    Value::Object(canonical)
}

fn canonicalize_filter_literal_list(
    value: &Value,
    model: &ManifestModel,
    input: &ManifestFilterInput,
    manifest: &ClientManifest,
) -> Value {
    match value {
        Value::List(items) => Value::List(
            items
                .iter()
                .map(|item| canonicalize_filter_literal(item, model, input, manifest))
                .collect(),
        ),
        Value::Null | Value::Variable(_) => value.clone(),
        value => Value::List(vec![canonicalize_filter_literal(
            value, model, input, manifest,
        )]),
    }
}

fn canonicalize_filter_comparison_literal(value: &Value, field: &ManifestField) -> Value {
    let Value::Object(operators) = value else {
        return value.clone();
    };
    let mut canonical = operators.clone();
    for (operator, operand) in operators {
        let operand = match operator.as_str() {
            "_in" | "_nin" => {
                let operand =
                    if matches!(operand, Value::List(_) | Value::Null | Value::Variable(_)) {
                        operand.clone()
                    } else {
                        Value::List(vec![operand.clone()])
                    };
                match operand {
                    Value::List(items) => Value::List(
                        items
                            .iter()
                            .map(|item| {
                                canonicalize_scalar_literal(item, &field.scalar, &field.codec)
                            })
                            .collect(),
                    ),
                    operand => operand,
                }
            }
            "_is_null" | "_has_key" => operand.clone(),
            _ => canonicalize_scalar_literal(operand, &field.scalar, &field.codec),
        };
        canonical.insert(operator.clone(), operand);
    }
    Value::Object(canonical)
}

fn canonicalize_scalar_literal(value: &Value, scalar: &str, codec: &str) -> Value {
    match (scalar, codec, value) {
        ("ID", "string", Value::Number(number)) if json_number_is_negative_zero(number) => {
            Value::String("0".into())
        }
        ("ID", "string", Value::Number(number)) if json_number_is_safe_integer(number) => {
            Value::String(number.to_string())
        }
        ("Bytea", "base64", Value::String(value)) => canonical_standard_base64(value)
            .map(Value::String)
            .unwrap_or_else(|| Value::String(value.clone())),
        ("Float", "float64", Value::Number(number)) => canonicalize_float_number(number)
            .map(Value::Number)
            .unwrap_or_else(|| Value::Number(number.clone())),
        ("Int", "int32", Value::Number(number))
        | ("BigInt", "json_number_precision_limited", Value::Number(number))
            if json_number_is_negative_zero(number) =>
        {
            Value::Number(serde_json::Number::from(0))
        }
        ("JSON", "json", value) => canonicalize_json_literal(value),
        _ => value.clone(),
    }
}

fn canonicalize_json_literal(value: &Value) -> Value {
    match value {
        Value::Number(number) => canonicalize_json_number(number)
            .map(Value::Number)
            .unwrap_or_else(|| Value::Number(number.clone())),
        Value::List(values) => Value::List(values.iter().map(canonicalize_json_literal).collect()),
        Value::Object(values) => Value::Object(
            values
                .iter()
                .map(|(name, value)| (name.clone(), canonicalize_json_literal(value)))
                .collect(),
        ),
        _ => value.clone(),
    }
}

fn canonicalize_json_number(number: &serde_json::Number) -> Option<serde_json::Number> {
    if json_number_is_negative_zero(number) {
        return Some(serde_json::Number::from(0));
    }
    if number.as_i64().is_some() || number.as_u64().is_some() {
        return Some(number.clone());
    }
    let value = number.as_f64().filter(|value| value.is_finite())?;
    if value.fract() == 0.0 {
        if value.abs() > 9_007_199_254_740_991.0 {
            return None;
        }
        return if value.is_sign_negative() {
            Some(serde_json::Number::from(value as i64))
        } else {
            Some(serde_json::Number::from(value as u64))
        };
    }
    serde_json::Number::from_f64(value)
}

fn canonicalize_float_number(number: &serde_json::Number) -> Option<serde_json::Number> {
    let value = number.as_f64()?;
    if value == 0.0 {
        Some(serde_json::Number::from(0))
    } else {
        serde_json::Number::from_f64(value)
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
    if let Some(codec) = &argument.codec {
        let valid = if argument.list {
            let Value::List(items) = value else {
                unreachable!("list shape checked above")
            };
            items.iter().all(|item| {
                value_to_json(item, document, position).is_ok_and(|item| {
                    scalar_json_literal_matches(&item, &argument.type_name, codec, false)
                })
            })
        } else {
            value_to_json(value, document, position).is_ok_and(|value| {
                scalar_json_literal_matches(&value, &argument.type_name, codec, false)
            })
        };
        return if valid {
            Ok(())
        } else {
            Err(source_error(
                "client.argument.literal_type",
                format!(
                    "literal for argument `{}` does not match scalar `{}` / codec `{}`",
                    argument.name, argument.type_name, codec
                ),
                document,
                position,
            ))
        };
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
            infer_route("src/routes/todos/+page.query.ts").as_deref(),
            Some("/todos")
        );
        assert_eq!(
            infer_route("/tmp/app/src/routes/+page.graphql").as_deref(),
            Some("/")
        );
        assert_eq!(
            infer_route("/tmp/app/src/routes/+page.query.ts").as_deref(),
            Some("/")
        );
        assert_eq!(infer_route("src/lib/todos.graphql"), None);
        assert_eq!(infer_route("src/routes/todos/query.graphql"), None);
        assert_eq!(infer_route("src/routes/todos/todos.query.ts"), None);
    }

    #[test]
    fn module_names_are_portable() {
        assert_eq!(module_stem("TodosForUser"), "todos-for-user");
        assert_eq!(module_stem("todos_for_user"), "todos-for-user");
    }
}
