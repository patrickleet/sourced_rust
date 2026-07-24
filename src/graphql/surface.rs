//! Shared GraphQL **surface IR** — single source of truth for the query/subscription
//! type system a catalog (and optionally a role) can see.
//!
//! SDL emission and (over time) runtime schema construction consume this IR so
//! dialect-honest comparison ops, roots, and column grants cannot diverge.
//!
//! Core types compile without the `graphql` feature so `dctl schema --format graphql`
//! can share the same IR path.

use std::collections::{BTreeMap, BTreeSet};

use sha2::{Digest, Sha256};

use super::command_contract::{
    compiled_direct_projection_target, compiled_projection_confirmation,
    validate_projection_confirmation_count, CommandConsistency, CommandDirectProjectionTarget,
    CommandEffect, CommandEffects, CommandInputDefault, CommandProjectedModel,
    CommandProjectionConfirmation, CompiledDirectProjectionTarget, CompiledProjectionConfirmation,
    EffectExpression, EffectFieldValue, EffectKey, EffectRelationship, TypedEffectKey,
};
use super::filter::{validate_row_policy_operand_literal, FilterExpr, Operand};
use crate::projection_protocol::ProjectionPartitionSpec;
use crate::projection_protocol::{compile_projection_topology, ProjectionModelOwnership};
use crate::table::{
    resolve_m2m_target_foreign_key, ColumnType, RelationshipKind, TableColumn, TableSchema,
};

use super::naming::{
    by_pk_field, comparison_exp_name, comparison_op_fields, include_postgres_json_comparison_ops,
    is_valid_graphql_name, object_type_name, reserved_type_names, root_list_field,
    scalar_type_name, CUSTOM_SCALARS,
};

/// Dialect gate for comparison operators (JSON ops only on Postgres).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceDialect {
    Sqlite,
    Postgres,
}

impl SurfaceDialect {
    pub fn is_postgres(self) -> bool {
        matches!(self, Self::Postgres)
    }
}

/// Options for building a surface from a table catalog.
#[derive(Clone, Debug)]
pub struct SurfaceOptions {
    pub dialect: SurfaceDialect,
    pub aggregates: bool,
    pub subscriptions: bool,
    /// Default page size used when a list request omits `limit`.
    pub default_limit: u64,
    /// Absolute page-size ceiling enforced by the query compiler.
    pub max_limit: u64,
}

impl SurfaceOptions {
    pub fn sqlite() -> Self {
        Self {
            dialect: SurfaceDialect::Sqlite,
            aggregates: true,
            subscriptions: true,
            default_limit: 100,
            max_limit: 1000,
        }
    }

    pub fn postgres() -> Self {
        Self {
            dialect: SurfaceDialect::Postgres,
            aggregates: true,
            subscriptions: true,
            default_limit: 100,
            max_limit: 1000,
        }
    }
}

/// Row-authorization semantics retained on the role/application surface.
///
/// `ServerOnly` is the fail-closed representation when a predicate differs
/// across application roles or references a field that is not authorized on
/// the selected surface. Clients may revalidate such collections but must not
/// evaluate membership locally.
#[derive(Clone, Debug, PartialEq)]
pub enum SurfaceRowPolicy {
    Unrestricted,
    Predicate(FilterExpr),
    ServerOnly,
}

/// Semantic category for one GraphQL field argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceArgumentKind {
    Filter,
    Order,
    Limit,
    Offset,
    PrimaryKey,
}

/// One accepted root/relationship argument from the shared Surface IR.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SurfaceArgument {
    pub name: String,
    pub kind: SurfaceArgumentKind,
    pub type_name: String,
    pub nullable: bool,
    pub list: bool,
}

/// Kind of a GraphQL root field on Query / Subscription.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootKind {
    List,
    ByPk,
    Aggregate,
}

/// One Query or Subscription root field inventory entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RootField {
    pub name: String,
    pub kind: RootKind,
    /// GraphQL object type name (`model_name`).
    pub object: String,
    /// Model name in the catalog (`schema.model_name`).
    pub model_name: String,
    pub arguments: Vec<SurfaceArgument>,
    /// Physical read-model dependencies used only for invalidation planning.
    pub dependencies: Vec<String>,
    pub default_limit: Option<u64>,
    pub max_limit: Option<u64>,
}

/// Column field on an object type (after skips / role filter).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnField {
    pub name: String,
    pub scalar: String,
    pub nullable: bool,
}

/// Relationship field inventory (target must be on the surface).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelField {
    pub name: String,
    pub target_model: String,
    pub target_object: String,
    pub kind: RelationshipKind,
    pub list: bool,
    /// Nullability of an object relationship. List relationships are always
    /// non-null lists and ignore this flag.
    pub nullable: bool,
    pub arguments: Vec<SurfaceArgument>,
    pub keys: SurfaceRelationshipKeys,
    pub dependencies: Vec<String>,
    pub aggregate: Option<SurfaceRelationshipAggregate>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SurfaceRelationshipAggregate {
    pub name: String,
    pub type_name: String,
    pub arguments: Vec<SurfaceArgument>,
    pub dependencies: Vec<String>,
}

/// Key/join metadata derived once from `RelationshipDef` while building the
/// Surface. Manifest and compiler consumers never walk the table catalog again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SurfaceRelationshipKeys {
    Direct {
        local: Vec<String>,
        remote: Vec<String>,
    },
    Through {
        local: Vec<String>,
        remote: Vec<String>,
        table: String,
        source_foreign_key: String,
        target_foreign_key: String,
    },
    /// Source/target identities are authorized, while the operational join
    /// table remains private. The opaque dependency is sufficient to mark
    /// cached relationship edges stale without exposing join internals.
    ThroughOpaque {
        local: Vec<String>,
        remote: Vec<String>,
        dependency: String,
    },
    /// Relationship remains server-queryable, but its local/client identity
    /// mapping is not authorized on this selected surface.
    Embedded,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SurfaceTypeField {
    pub name: String,
    pub type_name: String,
    pub nullable: bool,
    pub list: bool,
    pub item_nullable: bool,
    pub nested: Option<Box<SurfaceTypeDef>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SurfaceTypeDef {
    pub name: String,
    pub fields: Vec<SurfaceTypeField>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SurfaceCommandShape {
    None,
    Typed(SurfaceTypeDef),
}

/// Structural command mutation carried by the same role-filtered Surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SurfaceCommand {
    pub command_name: String,
    pub field_name: String,
    pub roles: Vec<String>,
    pub input: SurfaceCommandShape,
    pub output: SurfaceCommandShape,
    pub consistency: CommandConsistency,
    pub(crate) input_defaults: Vec<CommandInputDefault>,
    pub(crate) effects: Option<CommandEffects>,
    pub(crate) confirmations: Vec<CommandProjectionConfirmation>,
    pub(crate) projected_model: Option<CommandProjectedModel>,
    pub(crate) direct_projection: Option<CommandDirectProjectionTarget>,
    /// Authorization selection erased at least one required confirmation.
    /// No hidden projector/model/key IDs may survive into client artifacts.
    pub(crate) confirmation_unavailable: bool,
}

/// Projector topology declaration. Typed consistency/effects live on each
/// executable command contract rather than in this topology record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SurfaceProjector {
    pub name: String,
    pub facts: Vec<String>,
    pub models: Vec<String>,
    pub dependencies: Vec<String>,
    pub(crate) change_epoch: Option<String>,
    pub(crate) partition: ProjectionPartitionSpec,
}

impl SurfaceProjector {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            facts: Vec::new(),
            models: Vec::new(),
            dependencies: Vec::new(),
            change_epoch: None,
            partition: ProjectionPartitionSpec::unit(),
        }
    }

    pub fn facts(mut self, facts: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.facts = facts.into_iter().map(Into::into).collect();
        self
    }

    pub fn models(mut self, models: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.models = models.into_iter().map(Into::into).collect();
        self
    }

    /// Register the opaque change-log epoch owned by this projector topology.
    ///
    /// Epoch contents have no ordering meaning. They fence live resume and
    /// same-transaction record-change evidence across projector rebuilds.
    pub fn change_epoch(mut self, epoch: impl Into<String>) -> Self {
        self.change_epoch = Some(epoch.into());
        self
    }

    /// Derive a stable projection partition from one raw event JSON path.
    ///
    /// This closed declaration is evaluated before typed event decoding and is
    /// hashed into the durable topology. Reuse this exact projector value for
    /// GraphQL/direct binding and the asynchronous runtime.
    pub fn partition_by(mut self, path: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.partition = ProjectionPartitionSpec::input_path(path);
        self
    }

    /// Use one deterministic constant partition (including explicit JSON null).
    pub fn partition_constant(mut self, value: serde_json::Value) -> Self {
        self.partition = ProjectionPartitionSpec::constant(value);
        self
    }

    /// Reuse this exact topology declaration in a typed command confirmation
    /// plan. `command_confirmations!` calls this hidden seam so applications do
    /// not repeat projector or model IDs as strings.
    #[doc(hidden)]
    pub fn __distributed_confirmation<I, M: crate::read_model::RelationalReadModel>(
        &self,
        key: TypedEffectKey<M>,
    ) -> CompiledProjectionConfirmation<I> {
        compiled_projection_confirmation(
            &self.name,
            &self.facts,
            &self.models,
            &self.partition,
            key,
        )
    }

    /// Compiler seam for binding one `Projected<M>` command to this exact
    /// registered topology. Ordinary handlers never receive or construct it.
    #[doc(hidden)]
    pub fn __distributed_direct_projection<I, M>(&self) -> CompiledDirectProjectionTarget<I, M>
    where
        M: crate::read_model::RelationalReadModel + 'static,
    {
        compiled_direct_projection_target(
            &self.name,
            &self.facts,
            &self.models,
            &self.partition,
            self.change_epoch.as_deref(),
        )
    }
}

/// One exposed read-model on the surface.
#[derive(Clone)]
pub struct SurfaceModel {
    pub model_name: String,
    pub table_name: String,
    pub object_name: String,
    pub columns: Vec<ColumnField>,
    pub relationships: Vec<RelField>,
    pub primary_key: Vec<String>,
    pub row_policy: SurfaceRowPolicy,
    pub role_limit: Option<u64>,
    pub aggregations: bool,
    /// Filtered schema clone (columns limited for role surfaces).
    pub(crate) schema: TableSchema,
}

/// Whether this selected Surface exposes a complete client-safe normalized
/// identity for the model. Both manifest normalization and keyed optimistic
/// effects use this single predicate so an embedded model can never receive an
/// operation that assumes a stable normalized cache key.
pub(crate) fn model_has_client_normalized_identity(model: &SurfaceModel) -> bool {
    !model.primary_key.is_empty()
        && model.primary_key.iter().all(|key| {
            model
                .columns
                .iter()
                .find(|column| column.name == *key)
                .is_some_and(|column| {
                    !column.nullable
                        && column.scalar != "BigInt"
                        && matches!(
                            column.scalar.as_str(),
                            "Boolean"
                                | "Bytea"
                                | "Float"
                                | "ID"
                                | "Int"
                                | "JSON"
                                | "String"
                                | "Timestamptz"
                        )
                })
        })
}

/// Intermediate surface IR.
#[derive(Clone)]
pub struct Surface {
    pub(crate) selection: SurfaceSelection,
    // Structural fields stay crate-private so an authorized Surface cannot be
    // mutated after selection and then exported under its original role/app
    // provenance. Public consumers inspect derived artifacts or the read-only
    // helpers below; only the selection/compiler pipeline may change the IR.
    pub(crate) dialect: SurfaceDialect,
    pub(crate) aggregates: bool,
    pub(crate) subscriptions: bool,
    pub(crate) default_limit: u64,
    pub(crate) max_limit: u64,
    /// Complete validated table catalog, including operational relationship
    /// targets. It stays private and is carried through selection solely for
    /// shared policy/topology validation; manifests never serialize it.
    pub(crate) catalog: BTreeMap<String, TableSchema>,
    /// Keyed by `model_name`.
    pub(crate) models: BTreeMap<String, SurfaceModel>,
    pub(crate) query_fields: Vec<RootField>,
    pub(crate) subscription_fields: Vec<RootField>,
    /// GraphQL comparison input name → operator field names (from `naming` only).
    pub(crate) comparison_ops: BTreeMap<String, Vec<String>>,
    pub(crate) commands: Vec<SurfaceCommand>,
    /// Distinguishes an explicitly attached empty registry from a Surface that
    /// has not selected its one authoritative command source yet.
    pub(crate) commands_attached: bool,
    pub(crate) projectors: Vec<SurfaceProjector>,
    pub(crate) projectors_attached: bool,
    /// Non-serializable provenance proving typed commands came from one
    /// executable Service inventory rather than a lookalike command list.
    pub(crate) service_binding: Option<super::command_contract::TypedServiceCommandBinding>,
}

/// Debug output is intentionally limited to already-authorized public IDs.
/// The private catalog and filtered schema clones may contain denied names and
/// must never become an authorization side channel through derived formatting.
impl std::fmt::Debug for Surface {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Surface")
            .field("selection", &self.selection)
            .field("dialect", &self.dialect)
            .field("models", &self.models.keys().collect::<Vec<_>>())
            .field("query_roots", &self.query_root_names())
            .field("commands", &self.commands)
            .field("projectors", &self.projectors)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SurfaceSelection {
    Catalog,
    Role { name: String },
    Application { name: String, roles: Vec<String> },
}

impl Surface {
    /// Inventory of query root field names (sorted).
    pub fn query_root_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.query_fields.iter().map(|f| f.name.as_str()).collect();
        names.sort();
        names
    }

    /// Comparison operator fields for a scalar, empty if scalar unused.
    pub fn comparison_ops_for_scalar(&self, scalar: &str) -> Vec<&str> {
        let name = comparison_exp_name(scalar);
        self.comparison_ops
            .get(&name)
            .map(|ops| ops.iter().map(String::as_str).collect())
            .unwrap_or_default()
    }

    pub fn commands(&self) -> &[SurfaceCommand] {
        &self.commands
    }

    pub fn projectors(&self) -> &[SurfaceProjector] {
        &self.projectors
    }

    /// Attach the crate-private typed command inventory to this unselected
    /// catalog surface. Public callers derive this exclusively via
    /// [`Surface::with_service`].
    pub(crate) fn with_typed_commands(
        mut self,
        commands: &super::commands::TypedCommandInventory,
    ) -> Result<Self, String> {
        if !matches!(self.selection, SurfaceSelection::Catalog) {
            return Err(
                "commands can only be attached to the unselected catalog Surface before authorization selection"
                    .into(),
            );
        }
        if self.service_binding.is_some() {
            return Err(
                "commands are frozen after attachment from the executable Service inventory".into(),
            );
        }
        if self.commands_attached {
            return Err("a command registry has already been attached to this Surface".into());
        }
        self.commands = commands.surface_commands();
        validate_and_canonicalize_commands(&self.models, &self.comparison_ops, &mut self.commands)?;
        if self.projectors_attached {
            bind_surface_direct_projection_targets(
                &mut self.commands,
                &self.projectors,
                &self.models,
            )?;
            validate_command_confirmation_topology(&self.commands, &self.projectors, &self.models)?;
        }
        self.commands_attached = true;
        Ok(self)
    }

    /// Pool-free authoritative typed command path. The executable Routes
    /// inventory supplies both GraphQL declarations and non-forgeable service
    /// provenance used by static client export.
    pub fn with_service(mut self, service: &crate::microsvc::Service) -> Result<Self, String> {
        if !matches!(self.selection, SurfaceSelection::Catalog) {
            return Err(
                "service commands can only be attached to the unselected catalog Surface before authorization selection"
                    .into(),
            );
        }
        if self.commands_attached {
            return Err(
                "service commands cannot replace an already attached command inventory".into(),
            );
        }
        let binding = service.typed_command_binding()?;
        let contracts = service.typed_command_contracts();
        let commands = super::commands::TypedCommandInventory::from_contracts(&contracts)?;
        self = self.with_typed_commands(&commands)?;
        self.service_binding = Some(binding);
        Ok(self)
    }

    #[cfg(feature = "graphql")]
    pub(crate) fn with_service_binding(
        mut self,
        binding: Option<super::command_contract::TypedServiceCommandBinding>,
    ) -> Self {
        self.service_binding = binding;
        self
    }

    /// Attach and validate projector topology against the already-built model
    /// graph, deriving physical dependencies exactly once.
    pub fn with_projectors(
        mut self,
        projectors: impl IntoIterator<Item = SurfaceProjector>,
    ) -> Result<Self, String> {
        if !matches!(self.selection, SurfaceSelection::Catalog) {
            return Err(
                "projectors can only be attached to the unselected catalog Surface before authorization selection"
                    .into(),
            );
        }
        let mut out = Vec::new();
        let mut names = BTreeSet::new();
        for mut projector in projectors {
            if projector.name.trim().is_empty() {
                return Err("projector name must not be empty".into());
            }
            if !names.insert(projector.name.clone()) {
                return Err(format!("duplicate projector name `{}`", projector.name));
            }
            if projector.facts.is_empty() {
                return Err(format!(
                    "projector `{}` must declare at least one fact",
                    projector.name
                ));
            }
            validate_nonempty_unique_ids(
                &projector.facts,
                &format!("projector `{}` fact", projector.name),
            )?;
            if projector.models.is_empty() {
                return Err(format!(
                    "projector `{}` must declare at least one model",
                    projector.name
                ));
            }
            validate_nonempty_unique_ids(
                &projector.models,
                &format!("projector `{}` model", projector.name),
            )?;
            if let Some(epoch) = projector.change_epoch.as_deref() {
                crate::projection_protocol::ProjectionEpoch::new(epoch).map_err(|error| {
                    format!(
                        "projector `{}` change-log epoch is invalid: {error}",
                        projector.name
                    )
                })?;
            }
            projector.partition.validate().map_err(|error| {
                format!(
                    "projector `{}` has invalid partition declaration: {error}",
                    projector.name
                )
            })?;
            projector.facts.sort();
            projector.models.sort();
            let mut dependencies = BTreeSet::new();
            for model in &projector.models {
                let Some(surface_model) = self.models.get(model) else {
                    return Err(format!(
                        "projector `{}` targets unknown surface model `{model}`",
                        projector.name
                    ));
                };
                dependencies.insert(surface_model.table_name.clone());
            }
            projector.dependencies = dependencies.into_iter().collect();
            out.push(projector);
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        bind_surface_direct_projection_targets(&mut self.commands, &out, &self.models)?;
        self.projectors = out;
        self.projectors_attached = true;
        validate_command_confirmation_topology(&self.commands, &self.projectors, &self.models)?;
        Ok(self)
    }
}

/// Build the full (unscoped) surface from a table catalog.
pub fn build_surface(tables: &[TableSchema], options: &SurfaceOptions) -> Result<Surface, String> {
    let mut catalog = BTreeMap::new();
    let mut all_table_ids = BTreeSet::new();
    for schema in tables {
        schema
            .validate()
            .map_err(|e| format!("schema `{}` invalid: {e}", schema.model_name))?;
        if schema.model_name.trim().is_empty() {
            return Err("table model id must not be empty".into());
        }
        if catalog
            .insert(schema.model_name.clone(), schema.clone())
            .is_some()
        {
            return Err(format!(
                "duplicate table model id `{}` in Surface catalog",
                schema.model_name
            ));
        }
        if !all_table_ids.insert(schema.table_name.clone()) {
            return Err(format!(
                "table id `{}` collides with another table in Surface catalog",
                schema.table_name
            ));
        }
    }
    let read_models: Vec<&TableSchema> = tables.iter().filter(|t| t.kind.is_read_model()).collect();

    let mut model_ids = BTreeSet::new();
    let mut table_ids = BTreeSet::new();
    let mut object_ids = BTreeSet::new();
    for schema in &read_models {
        if schema.model_name.trim().is_empty() {
            return Err("read model id must not be empty".into());
        }
        if !model_ids.insert(schema.model_name.clone()) {
            return Err(format!(
                "duplicate read model id `{}` in Surface inventory",
                schema.model_name
            ));
        }
        if !table_ids.insert(schema.table_name.clone()) {
            return Err(format!(
                "duplicate read-model table id `{}` in Surface inventory",
                schema.table_name
            ));
        }
        let object_id = object_type_name(schema).to_string();
        if !object_ids.insert(object_id.clone()) {
            return Err(format!(
                "duplicate GraphQL object id `{object_id}` in Surface inventory"
            ));
        }
        let mut relationship_ids = BTreeSet::new();
        for relationship in &schema.relationships {
            if relationship.field_name.trim().is_empty() {
                return Err(format!(
                    "model `{}` has a relationship with an empty field id",
                    schema.model_name
                ));
            }
            if !relationship_ids.insert(relationship.field_name.clone()) {
                return Err(format!(
                    "model `{}` declares duplicate relationship field `{}`",
                    schema.model_name, relationship.field_name
                ));
            }
            if matches!(relationship.kind, RelationshipKind::ManyToMany)
                && relationship.through.is_none()
            {
                return Err(format!(
                    "model `{}` relationship `{}` many-to-many must declare `through`",
                    schema.model_name, relationship.field_name
                ));
            }
        }
    }

    let by_model: BTreeMap<&str, &TableSchema> = read_models
        .iter()
        .map(|t| (t.model_name.as_str(), *t))
        .collect();
    // All tables (incl. operational / unexposed join tables) so m2m
    // relationship_emitted can resolve `through` for bool_exp + object fields.
    let by_table: BTreeMap<&str, &TableSchema> =
        tables.iter().map(|t| (t.table_name.as_str(), t)).collect();

    let postgres_json = include_postgres_json_comparison_ops(options.dialect.is_postgres());
    let mut used_scalars: BTreeSet<String> = BTreeSet::new();
    let mut models: BTreeMap<String, SurfaceModel> = BTreeMap::new();

    for schema in &read_models {
        let object_name = object_type_name(schema).to_string();
        if !is_valid_graphql_name(&object_name) {
            return Err(format!(
                "object type `{object_name}` is not a valid GraphQL name"
            ));
        }
        if !is_valid_graphql_name(root_list_field(schema)) {
            return Err(format!(
                "root field `{}` is not a valid GraphQL name",
                root_list_field(schema)
            ));
        }

        let mut columns = Vec::new();
        for col in visible_columns(schema) {
            if !is_valid_graphql_name(&col.column_name) {
                return Err(format!(
                    "model `{}` column `{}` is not a valid GraphQL name",
                    schema.model_name, col.column_name
                ));
            }
            let Some(scalar) = scalar_type_name(&col.column_type) else {
                return Err(format!(
                    "model `{}` column `{}` has unsupported type",
                    schema.model_name, col.column_name
                ));
            };
            used_scalars.insert(scalar.to_string());
            columns.push(ColumnField {
                name: col.column_name.clone(),
                scalar: scalar.to_string(),
                nullable: col.nullable,
            });
        }

        let mut relationships = Vec::new();
        for rel in &schema.relationships {
            if !is_valid_graphql_name(&rel.field_name) {
                return Err(format!(
                    "model `{}` relationship `{}` is not a valid GraphQL name",
                    schema.model_name, rel.field_name
                ));
            }
            if !relationship_emitted(schema, rel, &by_model, &by_table) {
                continue;
            }
            let target = by_model
                .get(rel.target_model.as_str())
                .expect("relationship_emitted");
            let list = matches!(
                rel.kind,
                RelationshipKind::HasMany | RelationshipKind::ManyToMany
            );
            let nullable = if matches!(rel.kind, RelationshipKind::BelongsTo) {
                schema
                    .columns
                    .iter()
                    .find(|column| {
                        rel.foreign_key.as_deref().is_some_and(|key| {
                            column.column_name == key || column.field_name == key
                        })
                    })
                    .map(|column| column.nullable)
                    .unwrap_or(true)
            } else {
                false
            };
            let (keys, mut dependencies) = relationship_keys(schema, rel, target, &by_table)?;
            dependencies.sort();
            dependencies.dedup();
            relationships.push(RelField {
                name: rel.field_name.clone(),
                target_model: rel.target_model.clone(),
                target_object: object_type_name(target).to_string(),
                kind: rel.kind.clone(),
                list,
                nullable,
                arguments: if list {
                    list_arguments(target)
                } else {
                    Vec::new()
                },
                keys,
                aggregate: (options.aggregates && list).then(|| SurfaceRelationshipAggregate {
                    name: format!("{}_aggregate", rel.field_name),
                    type_name: format!("{}_aggregate", target.table_name),
                    arguments: vec![SurfaceArgument {
                        name: "where".into(),
                        kind: SurfaceArgumentKind::Filter,
                        type_name: format!("{}_bool_exp", target.table_name),
                        nullable: true,
                        list: false,
                    }],
                    dependencies: dependencies.clone(),
                }),
                dependencies,
            });
        }

        models.insert(
            schema.model_name.clone(),
            SurfaceModel {
                model_name: schema.model_name.clone(),
                table_name: schema.table_name.clone(),
                object_name,
                columns,
                relationships,
                primary_key: schema.primary_key.columns.clone(),
                row_policy: SurfaceRowPolicy::Unrestricted,
                role_limit: None,
                aggregations: options.aggregates,
                schema: (*schema).clone(),
            },
        );
    }

    // Drop relationships whose target was not included (defensive).
    let model_keys: BTreeSet<String> = models.keys().cloned().collect();
    for model in models.values_mut() {
        model
            .relationships
            .retain(|r| model_keys.contains(&r.target_model));
    }

    sanitize_relationship_identity(&mut models);

    let aggregate_targets: BTreeMap<String, bool> = models
        .iter()
        .map(|(name, model)| (name.clone(), model.aggregations))
        .collect();
    for model in models.values_mut() {
        for relationship in &mut model.relationships {
            if !aggregate_targets
                .get(&relationship.target_model)
                .copied()
                .unwrap_or(false)
            {
                relationship.aggregate = None;
            } else if let Some(aggregate) = &mut relationship.aggregate {
                aggregate.dependencies = relationship.dependencies.clone();
            }
        }
    }

    let mut comparison_ops = BTreeMap::new();
    for scalar in &used_scalars {
        let ops = comparison_op_fields(scalar, postgres_json);
        comparison_ops.insert(
            comparison_exp_name(scalar),
            ops.into_iter().map(str::to_string).collect(),
        );
    }
    // Always reserve custom scalar names for naming collisions checks downstream.
    let _ = CUSTOM_SCALARS;

    let (query_fields, subscription_fields) = root_fields_for_models(
        &models,
        options.aggregates,
        options.subscriptions,
        options.default_limit,
        options.max_limit,
    );

    validate_root_ids(&query_fields, "query")?;
    validate_root_ids(&subscription_fields, "subscription")?;
    validate_generated_surface_names(&models, &comparison_ops)?;

    Ok(Surface {
        selection: SurfaceSelection::Catalog,
        dialect: options.dialect,
        aggregates: options.aggregates,
        subscriptions: options.subscriptions,
        default_limit: options.default_limit,
        max_limit: options.max_limit,
        catalog,
        models,
        query_fields,
        subscription_fields,
        comparison_ops,
        commands: Vec::new(),
        commands_attached: false,
        projectors: Vec::new(),
        projectors_attached: false,
        service_binding: None,
    })
}

/// Role grant used by [`surface_for_role`] (feature-free; maps from `ReadPermission`
/// when the `graphql` feature is enabled).
#[derive(Clone, Debug)]
pub struct RoleGrant {
    pub all_columns: bool,
    pub columns: BTreeSet<String>,
    pub aggregations: bool,
    pub row_policy: SurfaceRowPolicy,
    pub limit: Option<u64>,
}

impl RoleGrant {
    pub fn all_columns() -> Self {
        Self {
            all_columns: true,
            columns: BTreeSet::new(),
            aggregations: false,
            row_policy: SurfaceRowPolicy::Unrestricted,
            limit: None,
        }
    }

    pub fn columns<I: IntoIterator<Item = impl Into<String>>>(cols: I) -> Self {
        Self {
            all_columns: false,
            columns: cols.into_iter().map(Into::into).collect(),
            aggregations: false,
            row_policy: SurfaceRowPolicy::Unrestricted,
            limit: None,
        }
    }

    pub fn with_aggregations(mut self) -> Self {
        self.aggregations = true;
        self
    }

    pub fn rows(mut self, predicate: FilterExpr) -> Self {
        self.row_policy = SurfaceRowPolicy::Predicate(predicate);
        self
    }

    pub fn server_only_rows(mut self) -> Self {
        self.row_policy = SurfaceRowPolicy::ServerOnly;
        self
    }

    pub fn limit(mut self, limit: u64) -> Self {
        self.limit = Some(limit);
        self
    }

    pub fn allows_column(&self, name: &str) -> bool {
        self.all_columns || self.columns.contains(name)
    }
}

/// Build a role→grant map for one role from `(model_name, role) → grant` entries.
///
/// Entries whose role does not match are ignored. Used by export/SDL and engine
/// adapters that already store grants keyed by `(model, role)`.
pub fn role_grants_for_role(
    role: &str,
    model_role_grants: &BTreeMap<(String, String), RoleGrant>,
) -> BTreeMap<String, RoleGrant> {
    let mut out = BTreeMap::new();
    for ((model, r), grant) in model_role_grants {
        if r == role {
            out.insert(model.clone(), grant.clone());
        }
    }
    out
}

/// Apply role grants: drop ungranted models and columns (and relationships to
/// dropped models). Aggregate roots omitted when `aggregations` is false.
///
/// `grants`: map of model_name → grant for this role. Missing model = not granted.
/// Returns an error when a row policy contains a literal that cannot be
/// represented faithfully by the shared runtime/client contract.
pub fn surface_for_role(
    surface: &Surface,
    role: &str,
    grants: &BTreeMap<String, RoleGrant>,
) -> Result<Surface, String> {
    // Validate the complete declared topology before authorization filtering.
    // Only a projector hidden by a valid role selection may become
    // `confirmation_unavailable`; an omitted catalog topology is an error.
    validate_command_confirmation_topology(
        &surface.commands,
        &surface.projectors,
        &surface.models,
    )?;
    validate_role_grants(surface, role, grants)?;
    let mut models: BTreeMap<String, SurfaceModel> = BTreeMap::new();

    for (model_name, model) in &surface.models {
        let Some(grant) = grants.get(model_name) else {
            continue;
        };

        let allowed_cols: BTreeSet<String> = model
            .columns
            .iter()
            .filter(|c| grant.allows_column(&c.name))
            .map(|c| c.name.clone())
            .collect();

        let columns: Vec<ColumnField> = model
            .columns
            .iter()
            .filter(|c| allowed_cols.contains(&c.name))
            .cloned()
            .collect();

        let mut schema = model.schema.clone();
        for col in &mut schema.columns {
            if !col.skipped && !allowed_cols.contains(&col.column_name) {
                col.skipped = true;
            }
        }

        models.insert(
            model_name.clone(),
            SurfaceModel {
                model_name: model.model_name.clone(),
                table_name: model.table_name.clone(),
                object_name: model.object_name.clone(),
                columns,
                relationships: model.relationships.clone(),
                primary_key: model.primary_key.clone(),
                row_policy: grant.row_policy.clone(),
                role_limit: grant.limit,
                aggregations: grant.aggregations,
                schema,
            },
        );
    }

    // Relationships only if target model remains granted (collect keys first).
    let model_keys: BTreeSet<String> = models.keys().cloned().collect();
    for model in models.values_mut() {
        model
            .relationships
            .retain(|r| model_keys.contains(&r.target_model));
        let rel_names: BTreeSet<String> =
            model.relationships.iter().map(|r| r.name.clone()).collect();
        model
            .schema
            .relationships
            .retain(|r| model_keys.contains(&r.target_model) && rel_names.contains(&r.field_name));
    }

    validate_selected_composite_relationships(&models)?;
    sanitize_relationship_identity(&mut models);

    let aggregate_targets: BTreeMap<String, bool> = models
        .iter()
        .map(|(name, model)| (name.clone(), model.aggregations))
        .collect();
    for model in models.values_mut() {
        for relationship in &mut model.relationships {
            if !aggregate_targets
                .get(&relationship.target_model)
                .copied()
                .unwrap_or(false)
            {
                relationship.aggregate = None;
            } else if let Some(aggregate) = &mut relationship.aggregate {
                aggregate.dependencies = relationship.dependencies.clone();
            }
        }
    }

    // A row predicate is portable only when every referenced model/field is
    // present on this selected surface. Otherwise retain the authorization
    // fact as `ServerOnly` without leaking the hidden dependency.
    let model_names: Vec<String> = models.keys().cloned().collect();
    for model_name in model_names {
        let policy = models[&model_name].row_policy.clone();
        if let SurfaceRowPolicy::Predicate(predicate) = &policy {
            if !filter_is_surface_visible(predicate, &model_name, &models)
                || !predicate.is_client_portable()
            {
                models.get_mut(&model_name).expect("model key").row_policy =
                    SurfaceRowPolicy::ServerOnly;
            }
        }
    }

    let mut query_fields = Vec::new();
    let mut subscription_fields = Vec::new();
    for model in models.values() {
        let grant = grants.get(&model.model_name);
        let allow_agg = surface.aggregates && grant.is_some_and(|g| g.aggregations);
        let list = root_list_field(&model.schema).to_string();
        let by_pk = by_pk_field(&model.schema);
        query_fields.push(root_field(
            model,
            list.clone(),
            RootKind::List,
            surface.default_limit,
            surface.max_limit,
        ));
        let stable_key_visible = !model.primary_key.is_empty()
            && model
                .primary_key
                .iter()
                .all(|key| model.columns.iter().any(|column| column.name == *key));
        if stable_key_visible {
            query_fields.push(root_field(
                model,
                by_pk.clone(),
                RootKind::ByPk,
                surface.default_limit,
                surface.max_limit,
            ));
        }
        if allow_agg {
            query_fields.push(root_field(
                model,
                format!("{}_aggregate", model.table_name),
                RootKind::Aggregate,
                surface.default_limit,
                surface.max_limit,
            ));
        }
        if surface.subscriptions {
            subscription_fields.push(root_field(
                model,
                list,
                RootKind::List,
                surface.default_limit,
                surface.max_limit,
            ));
        }
    }
    query_fields.sort_by(|a, b| a.name.cmp(&b.name));
    subscription_fields.sort_by(|a, b| a.name.cmp(&b.name));

    let postgres_json = include_postgres_json_comparison_ops(surface.dialect.is_postgres());
    let mut used_scalars: BTreeSet<String> = BTreeSet::new();
    for m in models.values() {
        for c in &m.columns {
            used_scalars.insert(c.scalar.clone());
        }
    }
    let mut comparison_ops = BTreeMap::new();
    for scalar in &used_scalars {
        let ops = comparison_op_fields(scalar, postgres_json);
        comparison_ops.insert(
            comparison_exp_name(scalar),
            ops.into_iter().map(str::to_string).collect(),
        );
    }

    let aggregates = query_fields.iter().any(|f| f.kind == RootKind::Aggregate);

    let mut commands: Vec<SurfaceCommand> = surface
        .commands
        .iter()
        .filter(|command| {
            command.roles.is_empty() || command.roles.iter().any(|allowed| allowed == role)
        })
        .cloned()
        .collect();
    for command in &mut commands {
        command.roles = vec![role.to_string()];
    }
    sanitize_command_effects_for_models(&mut commands, &models);

    let mut projectors = Vec::new();
    for projector in &surface.projectors {
        // Facts do not carry per-model provenance. If any target is denied,
        // retaining a subset would leak fact IDs/topology from that denied
        // domain, so omit the whole projector.
        if projector
            .models
            .iter()
            .any(|model| !models.contains_key(model))
        {
            continue;
        }
        let dependencies = projector
            .models
            .iter()
            .filter_map(|model| models.get(model).map(|model| model.table_name.clone()))
            .collect();
        projectors.push(SurfaceProjector {
            name: projector.name.clone(),
            facts: projector.facts.clone(),
            models: projector.models.clone(),
            dependencies,
            change_epoch: projector.change_epoch.clone(),
            partition: projector.partition.clone(),
        });
    }
    sanitize_command_confirmations(&mut commands, &projectors, &models);

    Ok(Surface {
        selection: SurfaceSelection::Role {
            name: role.to_string(),
        },
        dialect: surface.dialect,
        aggregates,
        subscriptions: surface.subscriptions,
        default_limit: surface.default_limit,
        max_limit: surface.max_limit,
        catalog: surface.catalog.clone(),
        models,
        query_fields,
        subscription_fields,
        comparison_ops,
        commands,
        commands_attached: surface.commands_attached,
        projectors,
        projectors_attached: surface.projectors_attached,
        service_binding: surface.service_binding.clone(),
    })
}

/// Composite identities are valid for isolated roots. Relationship compilation
/// still uses a single-column join contract, so reject the topology only when
/// both ends are reachable on this selected authorization surface. Keeping the
/// check here makes runtime role schemas and pool-free/dctl exports fail at the
/// same boundary without rejecting unrelated hidden catalog metadata.
fn validate_selected_composite_relationships(
    models: &BTreeMap<String, SurfaceModel>,
) -> Result<(), String> {
    for model in models.values() {
        let pk_n = model.primary_key.len();
        if pk_n <= 1 {
            continue;
        }
        let relationship_topology = !model.relationships.is_empty()
            || models.values().any(|candidate| {
                candidate
                    .relationships
                    .iter()
                    .any(|relationship| relationship.target_model == model.model_name)
            });
        if relationship_topology {
            return Err(format!(
                "model `{}` has a {pk_n}-column primary key and relationship topology; composite-key GraphQL models are supported only as isolated roots until composite relationship keys are implemented",
                model.model_name
            ));
        }
    }
    Ok(())
}

fn validate_role_grants(
    surface: &Surface,
    role: &str,
    grants: &BTreeMap<String, RoleGrant>,
) -> Result<(), String> {
    for (model, grant) in grants {
        let selected_model = surface
            .models
            .get(model)
            .ok_or_else(|| format!("permission for unknown model `{model}` in surface `{role}`"))?;

        if !grant.all_columns {
            for column in &grant.columns {
                if !selected_model
                    .schema
                    .columns
                    .iter()
                    .any(|candidate| candidate.column_name == *column && !candidate.skipped)
                {
                    return Err(format!(
                        "unknown column `{column}` in permission for `{model}` surface `{role}`"
                    ));
                }
            }
        }

        if let SurfaceRowPolicy::Predicate(predicate) = &grant.row_policy {
            predicate.validate_row_policy_literals().map_err(|error| {
                format!("invalid row policy for model `{model}` in surface `{role}`: {error}")
            })?;
            validate_surface_filter(
                predicate,
                &selected_model.schema,
                &surface.catalog,
                model,
                role,
            )?;
        }
    }
    Ok(())
}

/// Validate the executable row-policy graph against the same complete catalog
/// used by the runtime compiler. This deliberately permits policy references
/// to denied columns/models (the policy remains server-enforced) while rejecting
/// identifiers and relationship shapes that the runtime cannot compile.
fn validate_surface_filter(
    filter: &FilterExpr,
    schema: &TableSchema,
    catalog: &BTreeMap<String, TableSchema>,
    model: &str,
    role: &str,
) -> Result<(), String> {
    match filter {
        FilterExpr::And(items) | FilterExpr::Or(items) => {
            for item in items {
                validate_surface_filter(item, schema, catalog, model, role)?;
            }
        }
        FilterExpr::Not(item) => {
            validate_surface_filter(item, schema, catalog, model, role)?;
        }
        FilterExpr::Cmp { column, op, rhs } => {
            let column_schema = schema
                .columns
                .iter()
                .find(|candidate| candidate.column_name == *column)
                .ok_or_else(|| {
                    format!("unknown column `{column}` in filter for `{model}` surface `{role}`")
                })?;
            if matches!(column_schema.column_type, ColumnType::Json)
                && matches!(rhs, Operand::Claim(_))
            {
                return Err(format!(
                    "claims cannot compare to Json columns (`{column}` on `{model}`)"
                ));
            }
            validate_row_policy_operand_literal(column, &column_schema.column_type, Some(*op), rhs)
                .map_err(|error| {
                    format!("invalid row policy for model `{model}` surface `{role}`: {error}")
                })?;
        }
        FilterExpr::In { column, values, .. } => {
            let column_schema = schema
                .columns
                .iter()
                .find(|candidate| candidate.column_name == *column)
                .ok_or_else(|| {
                    format!("unknown column `{column}` in filter for `{model}` surface `{role}`")
                })?;
            for (index, value) in values.iter().enumerate() {
                validate_row_policy_operand_literal(
                    column,
                    &column_schema.column_type,
                    None,
                    value,
                )
                .map_err(|error| {
                        format!(
                            "invalid row policy for model `{model}` surface `{role}` IN operand {index}: {error}"
                        )
                    })?;
            }
        }
        FilterExpr::IsNull { column, .. } => {
            if !schema
                .columns
                .iter()
                .any(|candidate| candidate.column_name == *column)
            {
                return Err(format!(
                    "unknown column `{column}` in filter for `{model}` surface `{role}`"
                ));
            }
        }
        FilterExpr::Rel { field, predicate } => {
            let relationship = schema
                .relationships
                .iter()
                .find(|candidate| candidate.field_name == *field)
                .ok_or_else(|| {
                    format!("rel(`{field}`) is not a relationship on model `{model}`")
                })?;
            let target = catalog.get(&relationship.target_model).ok_or_else(|| {
                format!(
                    "rel(`{field}`) target `{}` is not in the catalog (model `{model}`)",
                    relationship.target_model
                )
            })?;

            if schema.primary_key.columns.len() > 1 || target.primary_key.columns.len() > 1 {
                return Err(format!(
                    "row policy for model `{model}` surface `{role}` traverses relationship `{field}` with composite-key topology; composite relationship keys are not implemented"
                ));
            }

            if matches!(relationship.kind, RelationshipKind::ManyToMany) {
                let through = relationship.through.as_deref().ok_or_else(|| {
                    format!("rel(`{field}`) many-to-many missing through on `{model}`")
                })?;
                if !catalog
                    .values()
                    .any(|candidate| candidate.table_name == through)
                {
                    return Err(format!(
                        "rel(`{field}`) through table `{through}` not in catalog"
                    ));
                }
            }
            validate_surface_filter(predicate, target, catalog, &relationship.target_model, role)?;
        }
    }
    Ok(())
}

/// Build an explicit named application surface as the structural intersection
/// of all runtime roles it supports.
///
/// A missing role declaration is an error rather than an accidental empty or
/// admin surface. Commands must be granted to every role; differing row
/// predicates become `ServerOnly`, so the client revalidates membership without
/// learning another role's policy.
pub fn surface_for_application(
    surface: &Surface,
    application: &str,
    roles: &[String],
    grants_by_role: &BTreeMap<String, BTreeMap<String, RoleGrant>>,
) -> Result<Surface, String> {
    if application.trim().is_empty() {
        return Err("application surface name must not be empty".into());
    }
    let mut roles = roles.to_vec();
    roles.sort();
    roles.dedup();
    if roles.is_empty() {
        return Err(format!(
            "application surface `{application}` must declare at least one role"
        ));
    }
    for role in &roles {
        let Some(grants) = grants_by_role.get(role) else {
            return Err(format!(
                "application surface `{application}` references undeclared role `{role}`"
            ));
        };
        // Validate every concrete role before intersecting it. Differing
        // predicates collapse to ServerOnly below, but that must not hide a
        // malformed identifier or unsupported relationship traversal.
        let _ = surface_for_role(surface, role, grants)?;
    }

    let mut common = BTreeMap::new();
    for (model_name, model) in &surface.models {
        let grants: Option<Vec<&RoleGrant>> = roles
            .iter()
            .map(|role| {
                grants_by_role
                    .get(role)
                    .and_then(|grants| grants.get(model_name))
            })
            .collect();
        let Some(grants) = grants else {
            continue;
        };

        let columns: BTreeSet<String> = model
            .columns
            .iter()
            .map(|column| column.name.clone())
            .filter(|column| grants.iter().all(|grant| grant.allows_column(column)))
            .collect();
        let aggregations = grants.iter().all(|grant| grant.aggregations);
        let first_policy = grants[0].row_policy.clone();
        let row_policy = if grants.iter().all(|grant| grant.row_policy == first_policy) {
            first_policy
        } else {
            SurfaceRowPolicy::ServerOnly
        };
        let limit = grants.iter().filter_map(|grant| grant.limit).min();
        common.insert(
            model_name.clone(),
            RoleGrant {
                all_columns: false,
                columns,
                aggregations,
                row_policy,
                limit,
            },
        );
    }

    let mut selected = surface_for_role(surface, application, &common)?;
    selected.commands = surface
        .commands
        .iter()
        .filter(|command| {
            command.roles.is_empty()
                || roles
                    .iter()
                    .all(|role| command.roles.iter().any(|allowed| allowed == role))
        })
        .cloned()
        .map(|mut command| {
            command.roles = roles.clone();
            command
        })
        .collect();
    sanitize_command_effects_for_models(&mut selected.commands, &selected.models);
    sanitize_command_confirmations(
        &mut selected.commands,
        &selected.projectors,
        &selected.models,
    );
    selected
        .commands
        .sort_by(|a, b| a.command_name.cmp(&b.command_name));
    selected.selection = SurfaceSelection::Application {
        name: application.to_string(),
        roles,
    };
    Ok(selected)
}

fn sanitize_relationship_identity(models: &mut BTreeMap<String, SurfaceModel>) {
    let visible_fields: BTreeMap<String, BTreeSet<String>> = models
        .iter()
        .map(|(name, model)| {
            (
                name.clone(),
                model
                    .columns
                    .iter()
                    .map(|field| field.name.clone())
                    .collect(),
            )
        })
        .collect();
    let visible_tables: BTreeMap<String, BTreeSet<String>> = models
        .values()
        .map(|model| {
            (
                model.table_name.clone(),
                model
                    .columns
                    .iter()
                    .map(|field| field.name.clone())
                    .collect(),
            )
        })
        .collect();

    for model in models.values_mut() {
        let source_fields = &visible_fields[&model.model_name];
        for relationship in &mut model.relationships {
            let target_fields = &visible_fields[&relationship.target_model];
            let keys_visible = match &relationship.keys {
                SurfaceRelationshipKeys::Direct { local, remote } => {
                    !local.is_empty()
                        && local.len() == remote.len()
                        && local.iter().all(|key| source_fields.contains(key))
                        && remote.iter().all(|key| target_fields.contains(key))
                }
                SurfaceRelationshipKeys::Through {
                    local,
                    remote,
                    table,
                    source_foreign_key,
                    target_foreign_key,
                } => {
                    let identity_visible = !local.is_empty()
                        && local.len() == remote.len()
                        && local.iter().all(|key| source_fields.contains(key))
                        && remote.iter().all(|key| target_fields.contains(key));
                    if identity_visible
                        && !visible_tables.get(table).is_some_and(|fields| {
                            fields.contains(source_foreign_key)
                                && fields.contains(target_foreign_key)
                        })
                    {
                        relationship.keys = SurfaceRelationshipKeys::ThroughOpaque {
                            local: local.clone(),
                            remote: remote.clone(),
                            dependency: opaque_relationship_dependency_id(
                                &model.model_name,
                                &relationship.name,
                                &relationship.target_model,
                            ),
                        };
                    }
                    identity_visible
                }
                SurfaceRelationshipKeys::ThroughOpaque { local, remote, .. } => {
                    !local.is_empty()
                        && local.len() == remote.len()
                        && local.iter().all(|key| source_fields.contains(key))
                        && remote.iter().all(|key| target_fields.contains(key))
                }
                SurfaceRelationshipKeys::Embedded => false,
            };
            if !keys_visible {
                relationship.keys = SurfaceRelationshipKeys::Embedded;
            }

            // Visible read-model tables are already public manifest IDs. Any
            // operational dependency (notably a private many-to-many join
            // table) remains useful for conservative invalidation but is
            // represented by a stable opaque ID instead of leaking its name.
            relationship.dependencies = relationship
                .dependencies
                .iter()
                .map(|dependency| {
                    if visible_tables.contains_key(dependency)
                        || dependency.starts_with("opaque:sha256:")
                    {
                        dependency.clone()
                    } else {
                        opaque_relationship_dependency_id(
                            &model.model_name,
                            &relationship.name,
                            &relationship.target_model,
                        )
                    }
                })
                .collect();
            relationship.dependencies.sort();
            relationship.dependencies.dedup();
            if let Some(aggregate) = &mut relationship.aggregate {
                // Aggregate invalidation must never retain the pre-sanitized
                // private dependency inventory.
                aggregate.dependencies = relationship.dependencies.clone();
            }
        }
    }
}

fn opaque_relationship_dependency_id(source: &str, relationship: &str, target: &str) -> String {
    let material = format!(
        "distributed.client.relationship-dependency.v1\0{source}\0{relationship}\0{target}\0join"
    );
    let digest = Sha256::digest(material.as_bytes());
    format!("opaque:sha256:{digest:x}")
}

fn validate_nonempty_unique_ids(values: &[String], label: &str) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for value in values {
        if value.trim().is_empty() {
            return Err(format!("{label} id must not be empty"));
        }
        if !seen.insert(value) {
            return Err(format!("duplicate {label} id `{value}`"));
        }
    }
    Ok(())
}

fn validate_root_ids(fields: &[RootField], operation: &str) -> Result<(), String> {
    let mut names = BTreeSet::new();
    for field in fields {
        if field.name.trim().is_empty() {
            return Err(format!("{operation} root id must not be empty"));
        }
        if !names.insert(&field.name) {
            return Err(format!(
                "duplicate {operation} root id `{}` in Surface inventory",
                field.name
            ));
        }
    }
    Ok(())
}

fn validate_generated_surface_names(
    models: &BTreeMap<String, SurfaceModel>,
    comparison_ops: &BTreeMap<String, Vec<String>>,
) -> Result<(), String> {
    let mut type_names: BTreeSet<String> = reserved_type_names().map(str::to_string).collect();
    let mut claim_type = |name: String| -> Result<(), String> {
        if !is_valid_graphql_name(&name) {
            return Err(format!(
                "generated type name `{name}` is not a valid GraphQL name"
            ));
        }
        if !type_names.insert(name.clone()) {
            return Err(format!(
                "generated type name `{name}` collides with another Surface type"
            ));
        }
        Ok(())
    };
    for name in comparison_ops.keys() {
        claim_type(name.clone())?;
    }
    for model in models.values() {
        claim_type(model.object_name.clone())?;
        claim_type(format!("{}_bool_exp", model.table_name))?;
        claim_type(format!("{}_order_by", model.table_name))?;
        if model.aggregations {
            claim_type(format!("{}_aggregate", model.table_name))?;
            claim_type(format!("{}_aggregate_fields", model.table_name))?;
        }

        let mut object_fields = BTreeSet::new();
        for column in &model.columns {
            if matches!(column.name.as_str(), "_and" | "_or" | "_not") {
                return Err(format!(
                    "model `{}` field `{}` collides with a generated boolean-expression field",
                    model.model_name, column.name
                ));
            }
            if !object_fields.insert(column.name.clone()) {
                return Err(format!(
                    "model `{}` has duplicate GraphQL object field `{}`",
                    model.model_name, column.name
                ));
            }
        }
        for relationship in &model.relationships {
            if matches!(relationship.name.as_str(), "_and" | "_or" | "_not") {
                return Err(format!(
                    "model `{}` relationship `{}` collides with a generated boolean-expression field",
                    model.model_name, relationship.name
                ));
            }
            if !object_fields.insert(relationship.name.clone()) {
                return Err(format!(
                    "model `{}` relationship `{}` collides with another object field",
                    model.model_name, relationship.name
                ));
            }
            if let Some(aggregate) = &relationship.aggregate {
                if !object_fields.insert(aggregate.name.clone()) {
                    return Err(format!(
                        "model `{}` relationship aggregate `{}` collides with another object field",
                        model.model_name, aggregate.name
                    ));
                }
            }
        }
    }
    Ok(())
}

fn validate_and_canonicalize_commands(
    models: &BTreeMap<String, SurfaceModel>,
    comparison_ops: &BTreeMap<String, Vec<String>>,
    commands: &mut [SurfaceCommand],
) -> Result<(), String> {
    let mut names = BTreeSet::new();
    let mut fields = BTreeSet::new();
    let mut type_defs: BTreeMap<String, (bool, SurfaceTypeDef)> = BTreeMap::new();
    let mut occupied_types: BTreeSet<String> = reserved_type_names().map(str::to_string).collect();
    occupied_types.extend(comparison_ops.keys().cloned());
    for model in models.values() {
        occupied_types.insert(model.object_name.clone());
        occupied_types.insert(format!("{}_bool_exp", model.table_name));
        occupied_types.insert(format!("{}_order_by", model.table_name));
        if model.aggregations {
            occupied_types.insert(format!("{}_aggregate", model.table_name));
            occupied_types.insert(format!("{}_aggregate_fields", model.table_name));
        }
    }
    for command in commands.iter_mut() {
        if command.command_name.trim().is_empty() {
            return Err("command id must not be empty".into());
        }
        if !names.insert(command.command_name.clone()) {
            return Err(format!("duplicate command id `{}`", command.command_name));
        }
        if !is_valid_graphql_name(&command.field_name) {
            return Err(format!(
                "command `{}` mutation field `{}` is not a valid GraphQL name",
                command.command_name, command.field_name
            ));
        }
        if !fields.insert(command.field_name.clone()) {
            return Err(format!(
                "duplicate command mutation field `{}`",
                command.field_name
            ));
        }
        validate_nonempty_unique_ids(
            &command.roles,
            &format!("command `{}` role", command.command_name),
        )?;
        command.roles.sort();
        match &mut command.input {
            SurfaceCommandShape::Typed(definition) => {
                canonicalize_type_def(definition)?;
                reject_occupied_command_types(definition, &occupied_types)?;
                register_type_def(definition, true, &mut type_defs)?;
            }
            SurfaceCommandShape::None => {}
        }
        let output_command_name = command.command_name.clone();
        let output_consistency = command.consistency;
        let output_projected_model = command.projected_model.clone();
        match &mut command.output {
            SurfaceCommandShape::None => {
                return Err(format!(
                    "command `{}` cannot declare an empty output",
                    command.command_name
                ));
            }
            SurfaceCommandShape::Typed(definition) => {
                canonicalize_type_def(definition)?;
                if projected_output_reuses_surface_model(
                    &output_command_name,
                    output_consistency,
                    output_projected_model.as_ref(),
                    definition,
                    models,
                )? {
                    // `Projected<M>` deliberately returns the already-exposed
                    // normalized model object. Do not claim or re-emit a second
                    // GraphQL type with the same name.
                } else {
                    reject_occupied_command_types(definition, &occupied_types)?;
                    register_type_def(definition, false, &mut type_defs)?;
                }
            }
        }
        command
            .input_defaults
            .sort_by(|left, right| left.path.cmp(&right.path));
        validate_command_input_defaults(command)?;
        validate_command_effects(models, command)?;
        validate_command_confirmations(models, command)?;
        command.confirmations.sort_by(|left, right| {
            serde_json::to_string(&left.canonical_value())
                .expect("confirmation IR serialization cannot fail")
                .cmp(
                    &serde_json::to_string(&right.canonical_value())
                        .expect("confirmation IR serialization cannot fail"),
                )
        });
    }
    commands.sort_by(|a, b| a.command_name.cmp(&b.command_name));
    Ok(())
}

pub(crate) fn projected_output_reuses_surface_model(
    command_name: &str,
    consistency: CommandConsistency,
    projected: Option<&CommandProjectedModel>,
    definition: &SurfaceTypeDef,
    models: &BTreeMap<String, SurfaceModel>,
) -> Result<bool, String> {
    if consistency != CommandConsistency::Projected {
        return Ok(false);
    }
    let Some(projected) = projected else {
        return Ok(false);
    };
    let Some(model) = models.get(&projected.model) else {
        return Ok(false);
    };
    if definition.name != model.object_name {
        return Ok(false);
    }
    if definition.fields.len() != model.columns.len() {
        return Err(format!(
            "typed projected command `{}` output `{}` does not match the normalized Surface model columns",
            command_name, definition.name
        ));
    }
    for field in &definition.fields {
        let Some(column) = model
            .columns
            .iter()
            .find(|column| column.name == field.name)
        else {
            return Err(format!(
                "typed projected command `{}` output `{}` contains non-model field `{}`",
                command_name, definition.name, field.name
            ));
        };
        if field.type_name != column.scalar
            || field.nullable != column.nullable
            || field.list
            || field.item_nullable
            || field.nested.is_some()
        {
            return Err(format!(
                "typed projected command `{}` output field `{}.{}` differs from its normalized Surface model column",
                command_name, definition.name, field.name
            ));
        }
    }
    Ok(true)
}

fn validate_command_input_defaults(command: &SurfaceCommand) -> Result<(), String> {
    if command.input_defaults.is_empty() {
        return Ok(());
    }
    let SurfaceCommandShape::Typed(input) = &command.input else {
        return Err(format!(
            "typed command `{}` declares generated input defaults on an untyped input",
            command.command_name
        ));
    };
    let mut paths = BTreeSet::new();
    for default in &command.input_defaults {
        if default.path.len() != 1 {
            return Err(format!(
                "typed command `{}` generated input default must target exactly one top-level field",
                command.command_name
            ));
        }
        if !paths.insert(default.path.clone()) {
            return Err(format!(
                "typed command `{}` repeats generated input default `{}`",
                command.command_name,
                default.path.join(".")
            ));
        }
        let field_name = &default.path[0];
        let field = input
            .fields
            .iter()
            .find(|field| field.name == *field_name)
            .ok_or_else(|| {
                format!(
                    "typed command `{}` generated input default references unknown field `{field_name}`",
                    command.command_name
                )
            })?;
        if field.nullable
            || field.list
            || field.nested.is_some()
            || !matches!(field.type_name.as_str(), "String" | "ID")
        {
            return Err(format!(
                "typed command `{}` generated input default `{field_name}` requires a non-null, non-list String/ID field",
                command.command_name
            ));
        }
    }
    Ok(())
}

fn validate_command_confirmations(
    models: &BTreeMap<String, SurfaceModel>,
    command: &SurfaceCommand,
) -> Result<(), String> {
    validate_projection_confirmation_count(&command.command_name, command.confirmations.len())?;
    match command.consistency {
        CommandConsistency::Fact if command.confirmations.is_empty() => {
            return Err(format!(
                "typed fact command `{}` must declare at least one expected projector confirmation",
                command.command_name
            ));
        }
        CommandConsistency::Projected if !command.confirmations.is_empty() => {
            return Err(format!(
                "typed projected command `{}` cannot declare asynchronous projector confirmations",
                command.command_name
            ));
        }
        CommandConsistency::Projected if command.projected_model.is_none() => {
            return Err(format!(
                "typed projected command `{}` is missing its compiler-retained relational model",
                command.command_name
            ));
        }
        CommandConsistency::Accepted | CommandConsistency::Fact
            if command.projected_model.is_some() || command.direct_projection.is_some() =>
        {
            return Err(format!(
                "typed non-projected command `{}` cannot carry direct projection metadata",
                command.command_name
            ));
        }
        _ => {}
    }
    if let Some(projected) = &command.projected_model {
        let model = models.get(&projected.model).ok_or_else(|| {
            format!(
                "typed projected command `{}` output references unknown model `{}`",
                command.command_name, projected.model
            )
        })?;
        if model.table_name != projected.table {
            return Err(format!(
                "typed projected command `{}` output model `{}` resolves to table `{}`, not `{}`",
                command.command_name, projected.model, model.table_name, projected.table
            ));
        }
        if let Some(partition) = &projected.partition {
            validate_effect_expression(
                command,
                partition,
                &ColumnField {
                    name: "projector partition".into(),
                    scalar: "String".into(),
                    nullable: false,
                },
            )?;
        }
    }
    if let Some(target) = &command.direct_projection {
        let model = models.get(&target.model).ok_or_else(|| {
            format!(
                "typed projected command `{}` targets unknown model `{}`",
                command.command_name, target.model
            )
        })?;
        if model.table_name != target.table {
            return Err(format!(
                "typed projected command `{}` target model `{}` resolves to table `{}`, not `{}`",
                command.command_name, target.model, model.table_name, target.table
            ));
        }
        if let Some(partition) = &target.partition {
            validate_effect_expression(
                command,
                partition,
                &ColumnField {
                    name: "projector partition".into(),
                    scalar: "String".into(),
                    nullable: false,
                },
            )?;
        }
    }
    if command.confirmation_unavailable {
        return Err(format!(
            "catalog command `{}` cannot start with an unavailable confirmation plan",
            command.command_name
        ));
    }

    let mut seen = BTreeSet::new();
    for confirmation in &command.confirmations {
        if confirmation.projector.trim().is_empty() {
            return Err(format!(
                "typed command `{}` confirmation projector must not be empty",
                command.command_name
            ));
        }
        validate_effect_key(models, command, &confirmation.model, &confirmation.key)?;
        if let Some(partition) = &confirmation.partition {
            validate_effect_expression(
                command,
                partition,
                &ColumnField {
                    name: "projector partition".into(),
                    scalar: "String".into(),
                    nullable: false,
                },
            )?;
        }
        let identity =
            serde_json::to_string(confirmation).expect("confirmation IR serialization cannot fail");
        if !seen.insert(identity) {
            return Err(format!(
                "typed command `{}` repeats an expected projector confirmation",
                command.command_name
            ));
        }
    }
    Ok(())
}

fn bind_surface_direct_projection_targets(
    commands: &mut [SurfaceCommand],
    projectors: &[SurfaceProjector],
    models: &BTreeMap<String, SurfaceModel>,
) -> Result<(), String> {
    let mut compiled_projectors = BTreeMap::new();
    for projector in projectors {
        let schemas = projector
            .models
            .iter()
            .map(|model_name| {
                models
                    .get(model_name)
                    .map(|model| &model.schema)
                    .ok_or_else(|| {
                        format!(
                            "projector `{}` references unknown model `{model_name}`",
                            projector.name
                        )
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let compiled = compile_projection_topology(
            &projector.name,
            &projector.facts,
            &projector.models,
            &projector.partition,
            schemas,
        )
        .map_err(|error| {
            format!(
                "projector `{}` has invalid compiled topology: {error}",
                projector.name
            )
        })?;
        compiled_projectors.insert(projector.name.clone(), compiled);
    }

    for command in commands {
        for confirmation in &mut command.confirmations {
            let projector = projectors
                .iter()
                .find(|projector| projector.name == confirmation.projector)
                .ok_or_else(|| {
                    format!(
                        "typed command `{}` expects unknown projector `{}`",
                        command.command_name, confirmation.projector
                    )
                })?;
            if !confirmation.topology_matches(
                &projector.name,
                &projector.facts,
                &projector.models,
                &projector.partition,
            ) {
                return Err(format!(
                    "typed command `{}` captured projector `{}` topology identity does not match the registered projector facts/models",
                    command.command_name, confirmation.projector
                ));
            }
            if !confirmation.partition_matches(&projector.partition) {
                return Err(format!(
                    "typed command `{}` confirmation for projector `{}` does not provide the partition mapping required by its declaration",
                    command.command_name, confirmation.projector
                ));
            }
            if !projector
                .models
                .iter()
                .any(|model| model == &confirmation.model)
            {
                return Err(format!(
                    "typed command `{}` expects projector `{}` to confirm model `{}`, but that model is not in the projector topology",
                    command.command_name, confirmation.projector, confirmation.model
                ));
            }
            let (topology, _) = compiled_projectors
                .get(&projector.name)
                .expect("every registered projector was compiled above");
            confirmation.bind_protocol_topology(topology.clone());
        }

        if command.consistency != CommandConsistency::Projected {
            continue;
        }
        let projected = command.projected_model.as_ref().ok_or_else(|| {
            format!(
                "typed projected command `{}` is missing its compiler-retained relational model",
                command.command_name
            )
        })?;
        let owners = projectors
            .iter()
            .filter(|projector| {
                projector
                    .models
                    .iter()
                    .any(|model| model == &projected.model)
            })
            .collect::<Vec<_>>();
        let projector = match owners.as_slice() {
            [projector] => *projector,
            [] => {
                return Err(format!(
                    "typed projected command `{}` output model `{}` has no registered SurfaceProjector owner",
                    command.command_name, projected.model
                ))
            }
            _ => {
                return Err(format!(
                    "typed projected command `{}` output model `{}` has ambiguous SurfaceProjector ownership: {}",
                    command.command_name,
                    projected.model,
                    owners
                        .iter()
                        .map(|owner| owner.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            }
        };
        if projector.change_epoch.is_none() {
            return Err(format!(
                "typed projected command `{}` owner `{}` has no registered change-log epoch",
                command.command_name, projector.name
            ));
        }
        let registered_schema = &models
            .get(&projected.model)
            .expect("projector ownership above requires a registered model")
            .schema;
        if projected.schema != registered_schema {
            return Err(format!(
                "typed projected command `{}` retained schema for `{}` differs from the registered full table schema",
                command.command_name, projected.model
            ));
        }
        if !projected.partition_matches(&projector.partition) {
            return Err(format!(
                "typed projected command `{}` does not provide the partition mapping required by projector `{}`",
                command.command_name, projector.name
            ));
        }
        let (protocol_topology, ownership) = compiled_projectors
            .get(&projector.name)
            .expect("every registered projector was compiled above");
        command.direct_projection = Some(projected.bind(
            &projector.name,
            &projector.facts,
            &projector.models,
            &projector.partition,
            projector.change_epoch.as_deref(),
            ownership.clone(),
            Some(protocol_topology.clone()),
        ));
    }
    Ok(())
}

fn validate_command_confirmation_topology(
    commands: &[SurfaceCommand],
    projectors: &[SurfaceProjector],
    models: &BTreeMap<String, SurfaceModel>,
) -> Result<(), String> {
    let mut compiled_projectors = BTreeMap::new();
    let mut physical_owners = BTreeMap::new();
    for projector in projectors {
        let schemas = projector
            .models
            .iter()
            .map(|model_name| {
                models
                    .get(model_name)
                    .map(|model| &model.schema)
                    .ok_or_else(|| {
                        format!(
                            "projector `{}` references unknown model `{model_name}`",
                            projector.name
                        )
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let compiled = compile_projection_topology(
            &projector.name,
            &projector.facts,
            &projector.models,
            &projector.partition,
            schemas,
        )
        .map_err(|error| {
            format!(
                "projector `{}` has invalid compiled topology: {error}",
                projector.name
            )
        })?;
        for owner in &compiled.1 {
            if let Some((existing_projector, existing_model)) = physical_owners.insert(
                owner.table.clone(),
                (projector.name.clone(), owner.model.clone()),
            ) {
                return Err(format!(
                    "physical table `{}` has multiple projector owners: `{existing_projector}`/`{existing_model}` and `{}`/`{}`",
                    owner.table, projector.name, owner.model
                ));
            }
        }
        compiled_projectors.insert(projector.name.clone(), compiled);
    }

    for command in commands {
        for confirmation in &command.confirmations {
            let projector = projectors
                .iter()
                .find(|projector| projector.name == confirmation.projector)
                .ok_or_else(|| {
                    format!(
                        "typed command `{}` expects unknown projector `{}`",
                        command.command_name, confirmation.projector
                    )
                })?;
            if !projector
                .models
                .iter()
                .any(|model| model == &confirmation.model)
            {
                return Err(format!(
                    "typed command `{}` expects projector `{}` to confirm model `{}`, but that model is not in the projector topology",
                    command.command_name, confirmation.projector, confirmation.model
                ));
            }
            if !confirmation.topology_matches(
                &projector.name,
                &projector.facts,
                &projector.models,
                &projector.partition,
            ) {
                return Err(format!(
                    "typed command `{}` captured projector `{}` topology identity does not match the registered projector facts/models",
                    command.command_name, confirmation.projector
                ));
            }
            let (expected_topology, _) = compiled_projectors
                .get(&projector.name)
                .expect("every registered projector was compiled above");
            if confirmation.protocol_topology() != Some(expected_topology) {
                return Err(format!(
                    "typed command `{}` confirmation for projector `{}` is not bound to the exact compiled schema topology",
                    command.command_name, confirmation.projector
                ));
            }
        }
        if let Some(target) = &command.direct_projection {
            let projector = projectors
                .iter()
                .find(|projector| projector.name == target.projector)
                .ok_or_else(|| {
                    format!(
                        "typed projected command `{}` expects unknown direct projector `{}`",
                        command.command_name, target.projector
                    )
                })?;
            if !projector.models.iter().any(|model| model == &target.model) {
                return Err(format!(
                    "typed projected command `{}` direct projector `{}` does not own model `{}`",
                    command.command_name, target.projector, target.model
                ));
            }
            if !target.topology_matches(
                &projector.name,
                &projector.facts,
                &projector.models,
                &projector.partition,
                projector.change_epoch.as_deref(),
            ) {
                return Err(format!(
                    "typed projected command `{}` captured direct projector `{}` topology/change epoch does not match the registered owner",
                    command.command_name, target.projector
                ));
            }
            let (expected_topology, _) = compiled_projectors
                .get(&projector.name)
                .expect("every registered projector was compiled above");
            if !target.protocol_topology_matches(expected_topology) {
                return Err(format!(
                    "typed projected command `{}` direct projector `{}` is not bound to the exact compiled schema topology",
                    command.command_name, target.projector
                ));
            }
            if projector.change_epoch.is_none() {
                return Err(format!(
                    "typed projected command `{}` direct projector `{}` has no registered change-log epoch",
                    command.command_name, target.projector
                ));
            }
            let mut expected_ownership = projector
                .models
                .iter()
                .map(|model_name| {
                    let model = models.get(model_name).ok_or_else(|| {
                        format!(
                            "typed projected command `{}` owner `{}` references unknown model `{model_name}`",
                            command.command_name, projector.name
                        )
                    })?;
                    ProjectionModelOwnership::new(model_name, &model.table_name)
                        .map_err(|error| error.to_string())
                })
                .collect::<Result<Vec<_>, _>>()?;
            expected_ownership.sort_by(|left, right| {
                (left.model.as_str(), left.table.as_str())
                    .cmp(&(right.model.as_str(), right.table.as_str()))
            });
            if target.ownership != expected_ownership {
                return Err(format!(
                    "typed projected command `{}` direct projector `{}` captured an incomplete or stale model/table ownership inventory",
                    command.command_name, target.projector
                ));
            }

            let physical_owners = projectors
                .iter()
                .flat_map(|candidate| {
                    candidate.models.iter().filter_map(move |model_name| {
                        models
                            .get(model_name)
                            .filter(|model| model.table_name == target.table)
                            .map(|_| (candidate.name.as_str(), model_name.as_str()))
                    })
                })
                .collect::<Vec<_>>();
            if physical_owners.as_slice() != [(target.projector.as_str(), target.model.as_str())] {
                return Err(format!(
                    "typed projected command `{}` model `{}` has ambiguous direct projection ownership",
                    command.command_name, target.model
                ));
            }
        }
    }
    Ok(())
}

fn validate_effect_key(
    models: &BTreeMap<String, SurfaceModel>,
    command: &SurfaceCommand,
    model_name: &str,
    key: &EffectKey,
) -> Result<(), String> {
    let model = models.get(model_name).ok_or_else(|| {
        format!(
            "typed command `{}` effect references unknown model `{model_name}`",
            command.command_name
        )
    })?;
    let fields: Vec<&str> = key
        .fields
        .iter()
        .map(|field| field.field.as_str())
        .collect();
    if fields
        != model
            .primary_key
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
    {
        return Err(format!(
            "typed command `{}` effect key for `{model_name}` must exactly match ordered primary key ({})",
            command.command_name,
            model.primary_key.join(", ")
        ));
    }
    for field in &key.fields {
        let Some(column) = model
            .columns
            .iter()
            .find(|column| column.name == field.field)
        else {
            return Err(format!(
                "typed command `{}` effect key for `{model_name}` references primary-key field `{}` that is missing or hidden on the selected Surface",
                command.command_name, field.field
            ));
        };
        validate_effect_expression(command, &field.value, column)?;
    }
    Ok(())
}

fn validate_command_effects(
    models: &BTreeMap<String, SurfaceModel>,
    command: &SurfaceCommand,
) -> Result<(), String> {
    let Some(effects) = &command.effects else {
        return Ok(());
    };
    for operation in &effects.operations {
        validate_effect_operation(models, command, operation)?;
    }
    Ok(())
}

fn validate_effect_operation(
    models: &BTreeMap<String, SurfaceModel>,
    command: &SurfaceCommand,
    operation: &CommandEffect,
) -> Result<(), String> {
    let validate_addressable = |model_name: &str| -> Result<(), String> {
        let model = models.get(model_name).ok_or_else(|| {
            format!(
                "typed command `{}` effect references unknown model `{model_name}`",
                command.command_name
            )
        })?;
        if !model_has_client_normalized_identity(model) {
            return Err(format!(
                "typed command `{}` cannot use a key-addressed optimistic effect for embedded model `{model_name}`; the selected Surface requires a complete visible, supported, non-null, non-BigInt primary key",
                command.command_name
            ));
        }
        Ok(())
    };
    let validate_fields = |model_name: &str, fields: &[EffectFieldValue]| -> Result<(), String> {
        let model = models.get(model_name).ok_or_else(|| {
            format!(
                "typed command `{}` effect references unknown model `{model_name}`",
                command.command_name
            )
        })?;
        let mut seen = BTreeSet::new();
        for field in fields {
            if !seen.insert(&field.field) {
                return Err(format!(
                    "typed command `{}` effect repeats `{model_name}.{}`",
                    command.command_name, field.field
                ));
            }
            let Some(column) = model
                .columns
                .iter()
                .find(|candidate| candidate.name == field.field)
            else {
                return Err(format!(
                    "typed command `{}` effect references unknown field `{model_name}.{}`",
                    command.command_name, field.field
                ));
            };
            if model.primary_key.iter().any(|key| key == &field.field) {
                return Err(format!(
                    "typed command `{}` effect cannot assign primary-key field `{model_name}.{}`; upsert identity materializes from `key` and rekeying is unsupported",
                    command.command_name, field.field
                ));
            }
            validate_effect_expression(command, &field.value, column)?;
        }
        Ok(())
    };
    let validate_relationship = |relationship: &EffectRelationship| -> Result<(), String> {
        let source = models.get(&relationship.source_model).ok_or_else(|| {
            format!(
                "typed command `{}` effect references unknown model `{}`",
                command.command_name, relationship.source_model
            )
        })?;
        let declared = source
            .relationships
            .iter()
            .find(|candidate| candidate.name == relationship.field)
            .ok_or_else(|| {
                format!(
                    "typed command `{}` effect references unknown relationship `{}.{}`",
                    command.command_name, relationship.source_model, relationship.field
                )
            })?;
        if declared.target_model != relationship.target_model {
            return Err(format!(
                "typed command `{}` relationship `{}.{}` targets `{}`, not `{}`",
                command.command_name,
                relationship.source_model,
                relationship.field,
                declared.target_model,
                relationship.target_model
            ));
        }
        Ok(())
    };

    match operation {
        CommandEffect::Upsert { model, key, fields }
        | CommandEffect::Patch { model, key, fields } => {
            validate_addressable(model)?;
            validate_effect_key(models, command, model, key)?;
            validate_fields(model, fields)
        }
        CommandEffect::Delete { model, key } => {
            validate_addressable(model)?;
            validate_effect_key(models, command, model, key)
        }
        CommandEffect::Link {
            relationship,
            source,
            target,
        }
        | CommandEffect::Unlink {
            relationship,
            source,
            target,
        } => {
            validate_relationship(relationship)?;
            validate_addressable(&relationship.source_model)?;
            validate_addressable(&relationship.target_model)?;
            validate_effect_key(models, command, &relationship.source_model, source)?;
            validate_effect_key(models, command, &relationship.target_model, target)
        }
        CommandEffect::InvalidateModel { model } => {
            if !models.contains_key(model) {
                return Err(format!(
                    "typed command `{}` invalidates unknown model `{model}`",
                    command.command_name
                ));
            }
            Ok(())
        }
        CommandEffect::InvalidateRelationship {
            relationship,
            source,
        } => {
            validate_relationship(relationship)?;
            validate_addressable(&relationship.source_model)?;
            validate_effect_key(models, command, &relationship.source_model, source)
        }
    }
}

fn validate_effect_expression(
    command: &SurfaceCommand,
    expression: &EffectExpression,
    expected: &ColumnField,
) -> Result<(), String> {
    match expression {
        EffectExpression::Input { path } => {
            let SurfaceCommandShape::Typed(input) = &command.input else {
                return Err(format!(
                    "typed command `{}` effect uses input on an untyped command",
                    command.command_name
                ));
            };
            if path.is_empty() {
                return Err(format!(
                    "typed command `{}` effect input path must not be empty",
                    command.command_name
                ));
            }
            let mut definition = input;
            let mut inherited_nullable = false;
            let mut leaf = None;
            for (index, segment) in path.iter().enumerate() {
                let Some(field) = definition
                    .fields
                    .iter()
                    .find(|field| field.name == *segment)
                else {
                    return Err(format!(
                        "typed command `{}` effect references unknown input path `{}`",
                        command.command_name,
                        path.join(".")
                    ));
                };
                let last = index + 1 == path.len();
                if last {
                    leaf = Some(field);
                    break;
                }
                if field.list {
                    return Err(format!(
                        "typed command `{}` effect input path `{}` cannot descend through list field `{}`",
                        command.command_name,
                        path.join("."),
                        segment
                    ));
                }
                inherited_nullable |= field.nullable;
                let Some(nested) = field.nested.as_deref() else {
                    return Err(format!(
                        "typed command `{}` effect input path `{}` descends through scalar field `{}`",
                        command.command_name,
                        path.join("."),
                        segment
                    ));
                };
                definition = nested;
            }
            let field = leaf.expect("non-empty input paths always resolve a leaf or return");
            let json_container_leaf =
                expected.scalar == "JSON" && (field.list || field.nested.is_some());
            if !json_container_leaf && (field.list || field.type_name != expected.scalar) {
                return Err(format!(
                    "typed command `{}` effect input `{}` has GraphQL type `{}`, but model field `{}` requires `{}`",
                    command.command_name,
                    path.join("."),
                    field.type_name,
                    expected.name,
                    expected.scalar
                ));
            }
            if (inherited_nullable || field.nullable) && !expected.nullable {
                return Err(format!(
                    "typed command `{}` nullable effect input `{}` cannot populate non-null model field `{}`",
                    command.command_name,
                    path.join("."),
                    expected.name
                ));
            }
        }
        EffectExpression::TrustedPreset { name } => {
            if name.is_empty()
                || name.len() > 128
                || name.trim() != name
                || name.chars().any(char::is_control)
            {
                return Err(format!(
                    "typed command `{}` trusted preset name must be 1..=128 bytes, have no surrounding whitespace, and contain no control characters",
                    command.command_name
                ));
            }
        }
        EffectExpression::Constant { value } => {
            let compatible = constant_matches_scalar(value, expected);
            if !compatible {
                return Err(format!(
                    "typed command `{}` constant effect value is incompatible with model field `{}` (`{}`)",
                    command.command_name, expected.name, expected.scalar
                ));
            }
        }
        EffectExpression::Null => {
            if !expected.nullable {
                return Err(format!(
                    "typed command `{}` null effect value cannot populate non-null model field `{}`",
                    command.command_name, expected.name
                ));
            }
        }
        EffectExpression::InvalidConstant { error } => {
            return Err(format!(
                "typed command `{}` constant effect value failed to serialize: {error}",
                command.command_name
            ));
        }
    }
    Ok(())
}

fn constant_matches_scalar(value: &serde_json::Value, expected: &ColumnField) -> bool {
    use base64::Engine as _;

    if expected.scalar == "JSON" {
        return true;
    }
    // `serde_json` represents non-finite floats as JSON null. SQL null has a
    // separate typed IR variant, so a constant null is invalid for every
    // non-JSON scalar even when the target column is nullable.
    if value.is_null() {
        return false;
    }
    match (expected.scalar.as_str(), value) {
        ("Boolean", serde_json::Value::Bool(_)) => true,
        ("BigInt", serde_json::Value::Number(number)) => number.is_i64() || number.is_u64(),
        ("Int", serde_json::Value::Number(number)) => {
            number
                .as_i64()
                .is_some_and(|value| i32::try_from(value).is_ok())
                || number
                    .as_u64()
                    .is_some_and(|value| i32::try_from(value).is_ok())
        }
        ("Float", serde_json::Value::Number(_)) => true,
        ("String" | "ID", serde_json::Value::String(_)) => true,
        ("Timestamptz", serde_json::Value::String(value)) => is_rfc3339_timestamp(value),
        ("Bytea", serde_json::Value::String(value)) => base64::engine::general_purpose::STANDARD
            .decode(value)
            .is_ok(),
        _ => false,
    }
}

/// Small dependency-free RFC 3339 validator for deterministic manifest
/// constants. Runtime database decoding remains dialect-owned.
fn is_rfc3339_timestamp(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() < 20
        || bytes.get(4) != Some(&b'-')
        || bytes.get(7) != Some(&b'-')
        || !matches!(bytes.get(10), Some(b'T' | b't'))
        || bytes.get(13) != Some(&b':')
        || bytes.get(16) != Some(&b':')
    {
        return false;
    }
    let digits = |range: std::ops::Range<usize>| -> Option<u32> {
        std::str::from_utf8(bytes.get(range)?).ok()?.parse().ok()
    };
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) = (
        digits(0..4),
        digits(5..7),
        digits(8..10),
        digits(11..13),
        digits(14..16),
        digits(17..19),
    ) else {
        return false;
    };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let max_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    if day == 0 || day > max_day || hour > 23 || minute > 59 || second > 60 {
        return false;
    }

    let mut cursor = 19;
    if bytes.get(cursor) == Some(&b'.') {
        cursor += 1;
        let fraction_start = cursor;
        while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
            cursor += 1;
        }
        if cursor == fraction_start {
            return false;
        }
    }
    match bytes.get(cursor) {
        Some(b'Z' | b'z') => cursor + 1 == bytes.len(),
        Some(b'+' | b'-') => {
            if cursor + 6 != bytes.len() || bytes.get(cursor + 3) != Some(&b':') {
                return false;
            }
            let offset_hour = std::str::from_utf8(&bytes[cursor + 1..cursor + 3])
                .ok()
                .and_then(|value| value.parse::<u32>().ok());
            let offset_minute = std::str::from_utf8(&bytes[cursor + 4..cursor + 6])
                .ok()
                .and_then(|value| value.parse::<u32>().ok());
            matches!((offset_hour, offset_minute), (Some(0..=23), Some(0..=59)))
        }
        _ => false,
    }
}

fn sanitize_command_effects_for_models(
    commands: &mut [SurfaceCommand],
    models: &BTreeMap<String, SurfaceModel>,
) {
    for command in commands {
        let Some(effects) = &command.effects else {
            continue;
        };
        if effects
            .operations
            .iter()
            .any(|operation| !effect_operation_visible(operation, models))
        {
            // Keep the command, but erase the entire optimistic plan. Partial
            // optimism is harder to reason about than conservative revalidation
            // and could leak denied model/field/preset names.
            command.effects = Some(CommandEffects::revalidate());
        }
    }
}

fn sanitize_command_confirmations(
    commands: &mut [SurfaceCommand],
    projectors: &[SurfaceProjector],
    models: &BTreeMap<String, SurfaceModel>,
) {
    for command in commands {
        let all_visible = command.confirmations.iter().all(|confirmation| {
            models.contains_key(&confirmation.model)
                && projectors.iter().any(|projector| {
                    projector.name == confirmation.projector
                        && projector
                            .models
                            .iter()
                            .any(|model| model == &confirmation.model)
                })
                && confirmation.key.fields.iter().all(|field| {
                    models.get(&confirmation.model).is_some_and(|model| {
                        model
                            .columns
                            .iter()
                            .any(|column| column.name == field.field)
                    }) && effect_expression_visible(&field.value)
                })
                && confirmation
                    .partition
                    .as_ref()
                    .is_none_or(effect_expression_visible)
        });
        if !all_visible {
            command.confirmations.clear();
            command.confirmation_unavailable = true;
            // Optimism and causal confirmation must be authorized as one plan.
            // Keeping either a subset of edges or a partial optimistic write
            // could disclose hidden topology and produce a state the server
            // never promised to confirm.
            command.effects = Some(CommandEffects::revalidate());
        }
    }
}

fn effect_operation_visible(
    operation: &CommandEffect,
    models: &BTreeMap<String, SurfaceModel>,
) -> bool {
    let key_visible = |model_name: &str, key: &EffectKey| {
        models.get(model_name).is_some_and(|model| {
            key.fields.iter().all(|field| {
                model
                    .columns
                    .iter()
                    .any(|column| column.name == field.field)
                    && effect_expression_visible(&field.value)
            })
        })
    };
    let fields_visible = |model_name: &str, fields: &[EffectFieldValue]| {
        models.get(model_name).is_some_and(|model| {
            fields.iter().all(|field| {
                model
                    .columns
                    .iter()
                    .any(|column| column.name == field.field)
                    && effect_expression_visible(&field.value)
            })
        })
    };
    let relationship_visible = |relationship: &EffectRelationship| {
        models.get(&relationship.source_model).is_some_and(|model| {
            model.relationships.iter().any(|candidate| {
                candidate.name == relationship.field
                    && candidate.target_model == relationship.target_model
                    && models.contains_key(&relationship.target_model)
            })
        })
    };
    match operation {
        CommandEffect::Upsert { model, key, fields }
        | CommandEffect::Patch { model, key, fields } => {
            key_visible(model, key) && fields_visible(model, fields)
        }
        CommandEffect::Delete { model, key } => key_visible(model, key),
        CommandEffect::Link {
            relationship,
            source,
            target,
        }
        | CommandEffect::Unlink {
            relationship,
            source,
            target,
        } => {
            relationship_visible(relationship)
                && key_visible(&relationship.source_model, source)
                && key_visible(&relationship.target_model, target)
        }
        CommandEffect::InvalidateModel { model } => models.contains_key(model),
        CommandEffect::InvalidateRelationship {
            relationship,
            source,
        } => relationship_visible(relationship) && key_visible(&relationship.source_model, source),
    }
}

fn effect_expression_visible(expression: &EffectExpression) -> bool {
    // A selected client surface may expose the descriptor name, but never its
    // value. Runtime values are read from the verified Session and travel only
    // in the cache-scope-bound protocol envelope.
    !matches!(expression, EffectExpression::InvalidConstant { .. })
}

fn reject_occupied_command_types(
    definition: &SurfaceTypeDef,
    occupied_types: &BTreeSet<String>,
) -> Result<(), String> {
    if occupied_types.contains(&definition.name) {
        return Err(format!(
            "command type `{}` collides with a Surface GraphQL type",
            definition.name
        ));
    }
    for field in &definition.fields {
        if let Some(nested) = &field.nested {
            reject_occupied_command_types(nested, occupied_types)?;
        }
    }
    Ok(())
}

fn canonicalize_type_def(definition: &mut SurfaceTypeDef) -> Result<(), String> {
    if !is_valid_graphql_name(&definition.name) {
        return Err(format!(
            "command type `{}` is not a valid GraphQL name",
            definition.name
        ));
    }
    if definition.fields.is_empty() {
        return Err(format!(
            "command type `{}` must declare at least one field",
            definition.name
        ));
    }
    let mut fields = BTreeSet::new();
    for field in &mut definition.fields {
        if !is_valid_graphql_name(&field.name) {
            return Err(format!(
                "command type `{}` field `{}` is not a valid GraphQL name",
                definition.name, field.name
            ));
        }
        if !fields.insert(field.name.clone()) {
            return Err(format!(
                "command type `{}` declares duplicate field `{}`",
                definition.name, field.name
            ));
        }
        if !field.list && field.item_nullable {
            return Err(format!(
                "command type `{}` field `{}` marks non-list items nullable",
                definition.name, field.name
            ));
        }
        if let Some(nested) = &mut field.nested {
            canonicalize_type_def(nested)?;
            if field.type_name != nested.name {
                return Err(format!(
                    "command type `{}` field `{}` names `{}` but embeds `{}`",
                    definition.name, field.name, field.type_name, nested.name
                ));
            }
        } else if !is_command_scalar(&field.type_name) {
            return Err(format!(
                "command type `{}` field `{}` references unknown type `{}` without a structural definition",
                definition.name, field.name, field.type_name
            ));
        }
    }
    definition.fields.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(())
}

fn register_type_def(
    definition: &SurfaceTypeDef,
    input: bool,
    type_defs: &mut BTreeMap<String, (bool, SurfaceTypeDef)>,
) -> Result<(), String> {
    if let Some((existing_input, existing)) = type_defs.get(&definition.name) {
        if *existing_input != input || existing != definition {
            return Err(format!(
                "ambiguous duplicate command type id `{}`",
                definition.name
            ));
        }
    } else {
        type_defs.insert(definition.name.clone(), (input, definition.clone()));
    }
    for field in &definition.fields {
        if let Some(nested) = &field.nested {
            register_type_def(nested, input, type_defs)?;
        }
    }
    Ok(())
}

fn is_command_scalar(name: &str) -> bool {
    matches!(name, "Boolean" | "Float" | "ID" | "Int" | "String") || CUSTOM_SCALARS.contains(&name)
}

fn filter_is_surface_visible(
    predicate: &FilterExpr,
    model_name: &str,
    models: &BTreeMap<String, SurfaceModel>,
) -> bool {
    let Some(model) = models.get(model_name) else {
        return false;
    };
    match predicate {
        FilterExpr::And(items) | FilterExpr::Or(items) => items
            .iter()
            .all(|item| filter_is_surface_visible(item, model_name, models)),
        FilterExpr::Not(item) => filter_is_surface_visible(item, model_name, models),
        FilterExpr::Cmp { column, rhs, .. } => model
            .columns
            .iter()
            .find(|field| field.name == *column)
            .is_some_and(|field| policy_operand_is_client_typed(rhs, &field.scalar)),
        FilterExpr::In { column, values, .. } => model
            .columns
            .iter()
            .find(|field| field.name == *column)
            .is_some_and(|field| {
                values
                    .iter()
                    .all(|value| policy_operand_is_client_typed(value, &field.scalar))
            }),
        FilterExpr::IsNull { column, .. } => {
            model.columns.iter().any(|field| field.name == *column)
        }
        FilterExpr::Rel { field, predicate } => model
            .relationships
            .iter()
            .find(|relationship| relationship.name == *field)
            .is_some_and(|relationship| {
                matches!(
                    relationship.keys,
                    SurfaceRelationshipKeys::Direct { .. }
                        | SurfaceRelationshipKeys::Through { .. }
                ) && filter_is_surface_visible(predicate, &relationship.target_model, models)
            }),
    }
}

fn policy_operand_is_client_typed(operand: &Operand, scalar: &str) -> bool {
    !matches!(operand, Operand::Claim(_))
        || matches!(
            scalar,
            "BigInt" | "Boolean" | "Float" | "ID" | "Int" | "String" | "Timestamptz"
        )
}

fn root_fields_for_models(
    models: &BTreeMap<String, SurfaceModel>,
    aggregates: bool,
    subscriptions: bool,
    default_limit: u64,
    max_limit: u64,
) -> (Vec<RootField>, Vec<RootField>) {
    let mut query_fields = Vec::new();
    let mut subscription_fields = Vec::new();
    for model in models.values() {
        let list = root_list_field(&model.schema).to_string();
        let by_pk = by_pk_field(&model.schema);
        query_fields.push(root_field(
            model,
            list.clone(),
            RootKind::List,
            default_limit,
            max_limit,
        ));
        query_fields.push(root_field(
            model,
            by_pk.clone(),
            RootKind::ByPk,
            default_limit,
            max_limit,
        ));
        if aggregates {
            query_fields.push(root_field(
                model,
                format!("{}_aggregate", model.table_name),
                RootKind::Aggregate,
                default_limit,
                max_limit,
            ));
        }
        if subscriptions {
            subscription_fields.push(root_field(
                model,
                list,
                RootKind::List,
                default_limit,
                max_limit,
            ));
        }
    }
    query_fields.sort_by(|a, b| a.name.cmp(&b.name));
    subscription_fields.sort_by(|a, b| a.name.cmp(&b.name));
    (query_fields, subscription_fields)
}

fn root_field(
    model: &SurfaceModel,
    name: String,
    kind: RootKind,
    default_limit: u64,
    max_limit: u64,
) -> RootField {
    let arguments = match kind {
        RootKind::List => list_arguments(&model.schema),
        RootKind::ByPk => primary_key_arguments(&model.schema),
        RootKind::Aggregate => vec![SurfaceArgument {
            name: "where".into(),
            kind: SurfaceArgumentKind::Filter,
            type_name: format!("{}_bool_exp", model.table_name),
            nullable: true,
            list: false,
        }],
    };
    let is_windowed = matches!(kind, RootKind::List);
    let effective_max = model.role_limit.unwrap_or(max_limit).min(max_limit);
    let role_default = default_limit.min(effective_max);
    RootField {
        name,
        kind,
        object: model.object_name.clone(),
        model_name: model.model_name.clone(),
        arguments,
        dependencies: vec![model.table_name.clone()],
        default_limit: is_windowed.then_some(role_default),
        max_limit: is_windowed.then_some(effective_max),
    }
}

fn list_arguments(schema: &TableSchema) -> Vec<SurfaceArgument> {
    vec![
        SurfaceArgument {
            name: "where".into(),
            kind: SurfaceArgumentKind::Filter,
            type_name: format!("{}_bool_exp", schema.table_name),
            nullable: true,
            list: false,
        },
        SurfaceArgument {
            name: "order_by".into(),
            kind: SurfaceArgumentKind::Order,
            type_name: format!("{}_order_by", schema.table_name),
            nullable: true,
            list: true,
        },
        SurfaceArgument {
            name: "limit".into(),
            kind: SurfaceArgumentKind::Limit,
            type_name: "Int".into(),
            nullable: true,
            list: false,
        },
        SurfaceArgument {
            name: "offset".into(),
            kind: SurfaceArgumentKind::Offset,
            type_name: "Int".into(),
            nullable: true,
            list: false,
        },
    ]
}

fn primary_key_arguments(schema: &TableSchema) -> Vec<SurfaceArgument> {
    schema
        .primary_key
        .columns
        .iter()
        .filter_map(|name| {
            let column = schema
                .columns
                .iter()
                .find(|column| column.column_name == *name && !column.skipped)?;
            let scalar = scalar_type_name(&column.column_type)?;
            Some(SurfaceArgument {
                name: name.clone(),
                kind: SurfaceArgumentKind::PrimaryKey,
                type_name: scalar.into(),
                nullable: false,
                list: false,
            })
        })
        .collect()
}

fn relationship_keys(
    source: &TableSchema,
    relationship: &crate::table::RelationshipDef,
    target: &TableSchema,
    by_table: &BTreeMap<&str, &TableSchema>,
) -> Result<(SurfaceRelationshipKeys, Vec<String>), String> {
    let foreign_key = relationship.foreign_key.as_deref().ok_or_else(|| {
        format!(
            "model `{}` relationship `{}` is missing foreign_key",
            source.model_name, relationship.field_name
        )
    })?;
    let mut dependencies = vec![source.table_name.clone(), target.table_name.clone()];
    let keys = match relationship.kind {
        RelationshipKind::BelongsTo => SurfaceRelationshipKeys::Direct {
            local: vec![canonical_column_name(source, foreign_key, relationship)?],
            remote: target.primary_key.columns.clone(),
        },
        RelationshipKind::HasMany => SurfaceRelationshipKeys::Direct {
            local: source.primary_key.columns.clone(),
            remote: vec![canonical_column_name(target, foreign_key, relationship)?],
        },
        RelationshipKind::ManyToMany => {
            let through_name = relationship.through.as_deref().ok_or_else(|| {
                format!(
                    "model `{}` relationship `{}` is missing through table",
                    source.model_name, relationship.field_name
                )
            })?;
            let through = by_table.get(through_name).ok_or_else(|| {
                format!(
                    "model `{}` relationship `{}` references missing through table `{through_name}`",
                    source.model_name, relationship.field_name
                )
            })?;
            let target_foreign_key =
                resolve_m2m_target_foreign_key(source, relationship, through, target)
                    .map_err(|error| error.to_string())?;
            dependencies.push(through.table_name.clone());
            SurfaceRelationshipKeys::Through {
                local: source.primary_key.columns.clone(),
                remote: target.primary_key.columns.clone(),
                table: through.table_name.clone(),
                source_foreign_key: canonical_column_name(through, foreign_key, relationship)?,
                target_foreign_key: canonical_column_name(
                    through,
                    &target_foreign_key,
                    relationship,
                )?,
            }
        }
    };
    Ok((keys, dependencies))
}

fn canonical_column_name(
    schema: &TableSchema,
    reference: &str,
    relationship: &crate::table::RelationshipDef,
) -> Result<String, String> {
    schema
        .columns
        .iter()
        .find(|column| column.column_name == reference || column.field_name == reference)
        .map(|column| column.column_name.clone())
        .ok_or_else(|| {
            format!(
                "relationship `{}` key `{reference}` is not a field or column on model `{}`",
                relationship.field_name, schema.model_name
            )
        })
}

fn visible_columns(schema: &TableSchema) -> impl Iterator<Item = &TableColumn> {
    schema.columns.iter().filter(|c| !c.skipped)
}

fn relationship_emitted(
    schema: &TableSchema,
    rel: &crate::table::RelationshipDef,
    by_model: &BTreeMap<&str, &TableSchema>,
    by_table: &BTreeMap<&str, &TableSchema>,
) -> bool {
    let Some(target) = by_model.get(rel.target_model.as_str()) else {
        return false;
    };
    match rel.kind {
        RelationshipKind::HasMany | RelationshipKind::BelongsTo => true,
        RelationshipKind::ManyToMany => {
            let Some(through_name) = rel.through.as_deref() else {
                return false;
            };
            if let Some(through) = by_table.get(through_name) {
                resolve_m2m_target_foreign_key(schema, rel, through, target).is_ok()
            } else {
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use super::*;
    use crate::graphql::command_contract::{CommandEffects, TypedCommandContract};
    use crate::graphql::commands::TypedCommandInventory;
    use crate::graphql::{GraphqlTypeDef, GraphqlTypeField};
    use crate::table::{ColumnType, PrimaryKey, RelationshipDef, TableColumn, TableKind};

    fn orders() -> TableSchema {
        TableSchema {
            model_name: "OrderView".into(),
            table_name: "orders".into(),
            columns: vec![
                TableColumn {
                    primary_key: true,
                    ..TableColumn::new("order_id", "order_id", ColumnType::Text)
                },
                TableColumn::new("customer_id", "customer_id", ColumnType::Text),
                TableColumn::new("status", "status", ColumnType::Text),
                TableColumn {
                    jsonb: true,
                    ..TableColumn::new("meta", "meta", ColumnType::Json)
                },
            ],
            primary_key: PrimaryKey::new(["order_id"]),
            version_column: Some("_sourced_version".into()),
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships: Vec::new(),
            kind: TableKind::ReadModel,
        }
    }

    fn operational() -> TableSchema {
        TableSchema {
            model_name: "Outbox".into(),
            table_name: "outbox".into(),
            columns: vec![TableColumn {
                primary_key: true,
                ..TableColumn::new("id", "id", ColumnType::Text)
            }],
            primary_key: PrimaryKey::new(["id"]),
            version_column: None,
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships: Vec::new(),
            kind: TableKind::Operational,
        }
    }

    fn test_command(
        command_name: &str,
        field_name: &str,
        output: GraphqlTypeDef,
    ) -> TypedCommandContract {
        let input_type_id = TypeId::of::<String>();
        let output_type_id = TypeId::of::<()>();
        TypedCommandContract {
            name: command_name.into(),
            field_name: field_name.into(),
            roles: Vec::new(),
            input: GraphqlTypeDef::new(
                "TestCommandInput",
                vec![GraphqlTypeField {
                    name: "id".into(),
                    type_name: "String".into(),
                    nullable: false,
                    list: false,
                    item_nullable: false,
                    nested: None,
                }],
            )
            .with_type_id(input_type_id),
            output: output.with_type_id(output_type_id),
            input_type_id,
            output_type_id,
            consistency: CommandConsistency::Accepted,
            input_defaults: Vec::new(),
            effects: CommandEffects::revalidate(),
            confirmations: Vec::new(),
            projected_model: None,
            direct_projection: None,
        }
    }

    fn test_inventory(
        contracts: impl IntoIterator<Item = TypedCommandContract>,
    ) -> TypedCommandInventory {
        TypedCommandInventory::from_contracts(&contracts.into_iter().collect::<Vec<_>>()).unwrap()
    }

    #[test]
    fn build_surface_skips_operational_and_lists_roots() {
        let surface =
            build_surface(&[orders(), operational()], &SurfaceOptions::sqlite()).expect("surface");
        assert!(surface.models.contains_key("OrderView"));
        assert!(!surface.models.contains_key("Outbox"));
        let roots = surface.query_root_names();
        assert!(roots.contains(&"orders"));
        assert!(roots.contains(&"orders_by_pk"));
        assert!(roots.contains(&"orders_aggregate"));
    }

    #[test]
    fn sqlite_surface_omits_pg_json_comparison_ops() {
        let surface = build_surface(&[orders()], &SurfaceOptions::sqlite()).unwrap();
        let ops = surface.comparison_ops_for_scalar("JSON");
        assert!(ops.contains(&"_eq"));
        for forbidden in ["_contains", "_contained_in", "_has_key"] {
            assert!(
                !ops.contains(&forbidden),
                "SQLite must not expose {forbidden}"
            );
        }
    }

    #[test]
    fn postgres_surface_includes_pg_json_comparison_ops() {
        let surface = build_surface(&[orders()], &SurfaceOptions::postgres()).unwrap();
        let ops = surface.comparison_ops_for_scalar("JSON");
        for required in ["_contains", "_contained_in", "_has_key"] {
            assert!(ops.contains(&required), "Postgres missing {required}");
        }
    }

    #[test]
    fn surface_rejects_duplicate_stable_model_ids() {
        let error = build_surface(&[orders(), orders()], &SurfaceOptions::sqlite()).unwrap_err();
        assert!(
            error.contains("duplicate table model id `OrderView`"),
            "{error}"
        );
    }

    #[test]
    fn surface_rejects_model_and_generated_auxiliary_type_collision() {
        let mut colliding = orders();
        colliding.model_name = "orders_bool_exp".into();
        colliding.table_name = "other_orders".into();
        let error = build_surface(&[orders(), colliding], &SurfaceOptions::sqlite()).unwrap_err();
        assert!(
            error.contains("`orders_bool_exp` collides with another Surface type"),
            "{error}"
        );
    }

    #[test]
    fn projector_topology_rejects_duplicate_and_empty_ids() {
        let surface = build_surface(&[orders()], &SurfaceOptions::sqlite()).unwrap();
        let duplicate = surface
            .clone()
            .with_projectors([
                SurfaceProjector::new("orders")
                    .facts(["order.changed"])
                    .models(["OrderView"]),
                SurfaceProjector::new("orders")
                    .facts(["order.created"])
                    .models(["OrderView"]),
            ])
            .unwrap_err();
        assert!(duplicate.contains("duplicate projector name `orders`"));

        let empty_fact = surface
            .clone()
            .with_projectors([SurfaceProjector::new("orders")
                .facts([""])
                .models(["OrderView"])])
            .unwrap_err();
        assert!(empty_fact.contains("fact id must not be empty"));

        let empty_model = surface
            .clone()
            .with_projectors([SurfaceProjector::new("orders")
                .facts(["order.changed"])
                .models([""])])
            .unwrap_err();
        assert!(empty_model.contains("model id must not be empty"));

        let no_facts = surface
            .clone()
            .with_projectors([SurfaceProjector::new("orders").models(["OrderView"])])
            .unwrap_err();
        assert!(no_facts.contains("must declare at least one fact"));
        let no_models = surface
            .with_projectors([SurfaceProjector::new("orders").facts(["order.changed"])])
            .unwrap_err();
        assert!(no_models.contains("must declare at least one model"));
    }

    #[test]
    fn selected_surfaces_reject_command_and_projector_reattachment() {
        let full = build_surface(&[orders()], &SurfaceOptions::sqlite()).unwrap();
        let grants = BTreeMap::from([("OrderView".into(), RoleGrant::all_columns())]);
        let selected = surface_for_role(&full, "user", &grants).unwrap();
        assert!(selected
            .clone()
            .with_typed_commands(&TypedCommandInventory::empty())
            .unwrap_err()
            .contains("before authorization selection"));
        assert!(selected
            .with_projectors(Vec::<SurfaceProjector>::new())
            .unwrap_err()
            .contains("before authorization selection"));

        let grants_by_role = BTreeMap::from([("user".into(), grants)]);
        let application =
            surface_for_application(&full, "web", &["user".into()], &grants_by_role).unwrap();
        assert!(application
            .clone()
            .with_typed_commands(&TypedCommandInventory::empty())
            .unwrap_err()
            .contains("before authorization selection"));
        assert!(application
            .with_projectors(Vec::<SurfaceProjector>::new())
            .unwrap_err()
            .contains("before authorization selection"));
    }

    #[test]
    fn role_policy_rejects_non_finite_and_hides_js_unsafe_integers() {
        let full = build_surface(&[orders()], &SurfaceOptions::sqlite()).unwrap();
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let grants = BTreeMap::from([(
                "OrderView".into(),
                RoleGrant::all_columns().rows(super::super::col("status").eq(value)),
            )]);
            assert!(surface_for_role(&full, "user", &grants)
                .unwrap_err()
                .contains("must be finite"));
        }

        let mut integer_orders = orders();
        integer_orders.columns.push(TableColumn::new(
            "sequence",
            "sequence",
            ColumnType::Integer,
        ));
        let full = build_surface(&[integer_orders], &SurfaceOptions::sqlite()).unwrap();
        let grants = BTreeMap::from([(
            "OrderView".into(),
            RoleGrant::all_columns()
                .rows(super::super::col("sequence").eq(9_007_199_254_740_992_i64)),
        )]);
        let selected = surface_for_role(&full, "user", &grants).unwrap();
        assert_eq!(
            selected.models["OrderView"].row_policy,
            SurfaceRowPolicy::ServerOnly
        );
    }

    #[test]
    fn command_surface_rejects_duplicate_mutation_field_ids() {
        let output = GraphqlTypeDef::new(
            "TestCommandPayload",
            vec![GraphqlTypeField {
                name: "id".into(),
                type_name: "String".into(),
                nullable: false,
                list: false,
                item_nullable: false,
                nested: None,
            }],
        );
        let commands = test_inventory([
            test_command("order.create", "orders_write", output.clone()),
            test_command("order.replace", "orders_write", output),
        ]);
        let error = build_surface(&[orders()], &SurfaceOptions::sqlite())
            .unwrap()
            .with_typed_commands(&commands)
            .unwrap_err();
        assert!(error.contains("duplicate command mutation field `orders_write`"));
    }

    #[test]
    fn command_surface_rejects_empty_nested_and_surface_colliding_types() {
        let empty = test_inventory([test_command(
            "order.empty",
            "order_empty",
            GraphqlTypeDef::new("EmptyPayload", Vec::new()),
        )]);
        let error = build_surface(&[orders()], &SurfaceOptions::sqlite())
            .unwrap()
            .with_typed_commands(&empty)
            .unwrap_err();
        assert!(error.contains("must declare at least one field"), "{error}");

        let nested = test_inventory([test_command(
            "order.nested_empty",
            "order_nested_empty",
            GraphqlTypeDef::new(
                "OuterPayload",
                vec![GraphqlTypeField {
                    name: "inner".into(),
                    type_name: "InnerPayload".into(),
                    nullable: false,
                    list: false,
                    item_nullable: false,
                    nested: Some(Box::new(GraphqlTypeDef::new("InnerPayload", Vec::new()))),
                }],
            ),
        )]);
        let error = build_surface(&[orders()], &SurfaceOptions::sqlite())
            .unwrap()
            .with_typed_commands(&nested)
            .unwrap_err();
        assert!(error.contains("`InnerPayload` must declare at least one field"));

        let collision = test_inventory([test_command(
            "order.collision",
            "order_collision",
            GraphqlTypeDef::new(
                "OrderView",
                vec![GraphqlTypeField {
                    name: "order_id".into(),
                    type_name: "String".into(),
                    nullable: false,
                    list: false,
                    item_nullable: false,
                    nested: None,
                }],
            ),
        )]);
        let error = build_surface(&[orders()], &SurfaceOptions::sqlite())
            .unwrap()
            .with_typed_commands(&collision)
            .unwrap_err();
        assert!(error.contains("collides with a Surface GraphQL type"));
    }

    #[test]
    fn projected_output_reuse_and_sdl_emission_use_the_same_exact_predicate() {
        let schema: &'static TableSchema = Box::leak(Box::new(orders()));
        let projected_model = CommandProjectedModel {
            output_type_id: std::any::TypeId::of::<()>(),
            model: "OrderView".into(),
            table: "orders".into(),
            schema,
            partition: None,
        };
        let projected_command = |output: SurfaceTypeDef| SurfaceCommand {
            command_name: "order.projected".into(),
            field_name: "order_projected".into(),
            roles: Vec::new(),
            input: SurfaceCommandShape::None,
            output: SurfaceCommandShape::Typed(output),
            consistency: CommandConsistency::Projected,
            input_defaults: Vec::new(),
            effects: Some(CommandEffects::revalidate()),
            confirmations: Vec::new(),
            projected_model: Some(projected_model.clone()),
            direct_projection: None,
            confirmation_unavailable: false,
        };
        let one_string_field = |name: &str| SurfaceTypeDef {
            name: name.into(),
            fields: vec![SurfaceTypeField {
                name: "order_id".into(),
                type_name: "String".into(),
                nullable: false,
                list: false,
                item_nullable: false,
                nested: None,
            }],
        };

        let mut custom = build_surface(&[orders()], &SurfaceOptions::sqlite()).unwrap();
        custom.commands = vec![projected_command(one_string_field(
            "CustomProjectedPayload",
        ))];
        validate_and_canonicalize_commands(
            &custom.models,
            &custom.comparison_ops,
            &mut custom.commands,
        )
        .unwrap();
        let sdl = crate::graphql::sdl::graphql_sdl_from_surface(&custom).unwrap();
        assert!(
            sdl.contains("type CustomProjectedPayload {"),
            "a non-reused projected output must still be emitted: {sdl}"
        );

        let mut mismatched = build_surface(&[orders()], &SurfaceOptions::sqlite()).unwrap();
        mismatched.commands = vec![projected_command(one_string_field("OrderView"))];
        let error = validate_and_canonicalize_commands(
            &mismatched.models,
            &mismatched.comparison_ops,
            &mut mismatched.commands,
        )
        .unwrap_err();
        assert!(
            error.contains("does not match the normalized Surface model columns"),
            "{error}"
        );
    }

    #[test]
    fn surface_for_role_drops_ungranted_columns_and_models() {
        let full = build_surface(&[orders()], &SurfaceOptions::sqlite()).unwrap();
        let mut grants = BTreeMap::new();
        grants.insert(
            "OrderView".to_string(),
            RoleGrant::columns(["order_id", "status"]),
        );
        let role_surface = surface_for_role(&full, "user", &grants).unwrap();
        let model = role_surface.models.get("OrderView").expect("granted");
        let col_names: Vec<_> = model.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(col_names, vec!["order_id", "status"]);
        assert!(!col_names.contains(&"customer_id"));
        assert!(!col_names.contains(&"meta"));

        let empty = surface_for_role(&full, "anon", &BTreeMap::new()).unwrap();
        assert!(empty.models.is_empty());
        assert!(empty.query_fields.is_empty());
    }

    #[test]
    fn role_surface_erases_denied_effects_and_retains_valid_trusted_presets() {
        let mut full = build_surface(&[orders()], &SurfaceOptions::sqlite()).unwrap();
        let input = SurfaceTypeDef {
            name: "UpdateOrderInput".into(),
            fields: vec![
                SurfaceTypeField {
                    name: "order_id".into(),
                    type_name: "String".into(),
                    nullable: false,
                    list: false,
                    item_nullable: false,
                    nested: None,
                },
                SurfaceTypeField {
                    name: "customer_id".into(),
                    type_name: "String".into(),
                    nullable: false,
                    list: false,
                    item_nullable: false,
                    nested: None,
                },
            ],
        };
        let key = EffectKey {
            fields: vec![EffectFieldValue {
                field: "order_id".into(),
                value: EffectExpression::Input {
                    path: vec!["order_id".into()],
                },
            }],
        };
        let denied_field_command = SurfaceCommand {
            command_name: "order.assign_customer".into(),
            field_name: "order_assign_customer".into(),
            roles: Vec::new(),
            input: SurfaceCommandShape::Typed(input.clone()),
            output: SurfaceCommandShape::Typed(SurfaceTypeDef {
                name: "AssignCustomerPayload".into(),
                fields: vec![SurfaceTypeField {
                    name: "order_id".into(),
                    type_name: "String".into(),
                    nullable: false,
                    list: false,
                    item_nullable: false,
                    nested: None,
                }],
            }),
            consistency: CommandConsistency::Accepted,
            input_defaults: Vec::new(),
            effects: Some(CommandEffects::new([CommandEffect::Patch {
                model: "OrderView".into(),
                key: key.clone(),
                fields: vec![EffectFieldValue {
                    field: "customer_id".into(),
                    value: EffectExpression::Input {
                        path: vec!["customer_id".into()],
                    },
                }],
            }])),
            confirmations: Vec::new(),
            projected_model: None,
            direct_projection: None,
            confirmation_unavailable: false,
        };
        let trusted_preset_command = SurfaceCommand {
            command_name: "order.apply_preset".into(),
            field_name: "order_apply_preset".into(),
            roles: Vec::new(),
            input: SurfaceCommandShape::Typed(input),
            output: SurfaceCommandShape::Typed(SurfaceTypeDef {
                name: "ApplyPresetPayload".into(),
                fields: vec![SurfaceTypeField {
                    name: "order_id".into(),
                    type_name: "String".into(),
                    nullable: false,
                    list: false,
                    item_nullable: false,
                    nested: None,
                }],
            }),
            consistency: CommandConsistency::Accepted,
            input_defaults: Vec::new(),
            effects: Some(CommandEffects::new([CommandEffect::Patch {
                model: "OrderView".into(),
                key,
                fields: vec![EffectFieldValue {
                    field: "status".into(),
                    value: EffectExpression::TrustedPreset {
                        name: "tenant-secret".into(),
                    },
                }],
            }])),
            confirmations: Vec::new(),
            projected_model: None,
            direct_projection: None,
            confirmation_unavailable: false,
        };
        full.commands = vec![denied_field_command, trusted_preset_command];
        validate_and_canonicalize_commands(&full.models, &full.comparison_ops, &mut full.commands)
            .unwrap();

        let selected = surface_for_role(
            &full,
            "user",
            &BTreeMap::from([(
                "OrderView".into(),
                RoleGrant::columns(["order_id", "status"]),
            )]),
        )
        .unwrap();
        let denied = selected
            .commands
            .iter()
            .find(|command| command.command_name == "order.assign_customer")
            .expect("denied-field command");
        assert!(denied
            .effects
            .as_ref()
            .is_some_and(|effects| effects.operations.is_empty()));
        let trusted = selected
            .commands
            .iter()
            .find(|command| command.command_name == "order.apply_preset")
            .expect("trusted-preset command");
        assert!(trusted
            .effects
            .as_ref()
            .is_some_and(|effects| !effects.operations.is_empty()));

        let manifest = super::super::client_manifest_from_surface(
            "orders",
            super::super::ClientSurfaceIdentity::role("user"),
            &selected,
        )
        .unwrap();
        let denied = manifest
            .commands
            .iter()
            .find(|command| command.name == "order.assign_customer")
            .expect("denied-field manifest command");
        assert!(denied
            .extensions
            .effects
            .as_ref()
            .is_some_and(
                |effects| effects.operations.is_empty() && effects.fallback == "revalidate"
            ));
        assert!(denied.extensions.trusted_presets.is_empty());
        let denied_effects_json = serde_json::to_string(&denied.extensions.effects).unwrap();
        assert!(
            !denied_effects_json.contains("customer_id"),
            "{denied_effects_json}"
        );
        assert!(
            !denied_effects_json.contains("tenant-secret"),
            "{denied_effects_json}"
        );

        let trusted = manifest
            .commands
            .iter()
            .find(|command| command.name == "order.apply_preset")
            .expect("trusted-preset manifest command");
        assert_eq!(
            trusted.extensions.trusted_presets,
            vec![super::super::ClientTrustedPresetDescriptor {
                name: "tenant-secret".into(),
                codec: "string".into(),
            }]
        );
        let trusted_json = serde_json::to_string(trusted).unwrap();
        assert!(trusted_json.contains("tenant-secret"), "{trusted_json}");
        let preset_descriptors_json =
            serde_json::to_string(&trusted.extensions.trusted_presets).unwrap();
        assert!(
            !preset_descriptors_json.contains("\"value\":"),
            "{preset_descriptors_json}"
        );
    }

    #[test]
    fn pool_free_role_selection_rejects_invalid_grants_and_policy_references() {
        let full = build_surface(&[orders()], &SurfaceOptions::sqlite()).unwrap();

        let unknown_model = surface_for_role(
            &full,
            "user",
            &BTreeMap::from([("TypoView".into(), RoleGrant::all_columns())]),
        )
        .unwrap_err();
        assert!(unknown_model.contains("unknown model `TypoView`"));

        let unknown_column = surface_for_role(
            &full,
            "user",
            &BTreeMap::from([(
                "OrderView".into(),
                RoleGrant::columns(["order_id", "statuz"]),
            )]),
        )
        .unwrap_err();
        assert!(unknown_column.contains("unknown column `statuz` in permission"));

        let unknown_filter_column = surface_for_role(
            &full,
            "user",
            &BTreeMap::from([(
                "OrderView".into(),
                RoleGrant::all_columns().rows(super::super::col("statuz").eq("open")),
            )]),
        )
        .unwrap_err();
        assert!(unknown_filter_column.contains("unknown column `statuz` in filter"));

        let unknown_relationship = surface_for_role(
            &full,
            "user",
            &BTreeMap::from([(
                "OrderView".into(),
                RoleGrant::all_columns().rows(super::super::rel(
                    "customer",
                    super::super::col("id").eq("c1"),
                )),
            )]),
        )
        .unwrap_err();
        assert!(unknown_relationship.contains("is not a relationship on model `OrderView`"));
    }

    #[test]
    fn pool_free_role_selection_rejects_mistyped_row_policy_literals() {
        let full = build_surface(&[orders()], &SurfaceOptions::sqlite()).unwrap();
        let cmp_error = surface_for_role(
            &full,
            "user",
            &BTreeMap::from([(
                "OrderView".into(),
                RoleGrant::all_columns().rows(FilterExpr::Cmp {
                    column: "status".into(),
                    op: super::super::filter::CmpOp::Eq,
                    rhs: Operand::Lit(super::super::LitValue::Json(serde_json::json!("open"))),
                }),
            )]),
        )
        .unwrap_err();
        assert!(cmp_error.contains("literal kind `json`"), "{cmp_error}");
        assert!(cmp_error.contains("column `status`"), "{cmp_error}");

        let in_error = surface_for_role(
            &full,
            "user",
            &BTreeMap::from([(
                "OrderView".into(),
                RoleGrant::all_columns().rows(FilterExpr::In {
                    column: "status".into(),
                    values: vec![
                        Operand::from("open"),
                        Operand::Lit(super::super::LitValue::Json(serde_json::json!("closed"))),
                    ],
                    negated: false,
                }),
            )]),
        )
        .unwrap_err();
        assert!(in_error.contains("IN operand 1"), "{in_error}");
    }

    #[test]
    fn selected_surface_debug_does_not_leak_denied_schema_metadata() {
        let full = build_surface(&[orders()], &SurfaceOptions::sqlite()).unwrap();
        let selected = surface_for_role(
            &full,
            "user",
            &BTreeMap::from([(
                "OrderView".into(),
                RoleGrant::columns(["order_id", "status"]),
            )]),
        )
        .unwrap();

        let debug = format!("{selected:?}");
        assert!(debug.contains("OrderView"));
        assert!(!debug.contains("customer_id"), "{debug}");
        assert!(!debug.contains("meta"), "{debug}");
    }

    #[test]
    fn surface_for_role_omits_aggregate_without_grant() {
        let full = build_surface(&[orders()], &SurfaceOptions::sqlite()).unwrap();
        let mut grants = BTreeMap::new();
        grants.insert("OrderView".to_string(), RoleGrant::all_columns());
        let role_surface = surface_for_role(&full, "user", &grants).unwrap();
        let names = role_surface.query_root_names();
        assert!(names.contains(&"orders"));
        assert!(!names.contains(&"orders_aggregate"));

        let mut admin = BTreeMap::new();
        admin.insert(
            "OrderView".to_string(),
            RoleGrant::all_columns().with_aggregations(),
        );
        let admin_surface = surface_for_role(&full, "admin", &admin).unwrap();
        assert!(admin_surface
            .query_root_names()
            .contains(&"orders_aggregate"));
    }

    #[test]
    fn relationship_only_when_target_on_surface() {
        let parent = TableSchema {
            model_name: "ParentView".into(),
            table_name: "parents".into(),
            columns: vec![TableColumn {
                primary_key: true,
                ..TableColumn::new("id", "id", ColumnType::Text)
            }],
            primary_key: PrimaryKey::new(["id"]),
            version_column: None,
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships: vec![RelationshipDef {
                field_name: "children".into(),
                kind: RelationshipKind::HasMany,
                target_model: "ChildView".into(),
                foreign_key: Some("parent_id".into()),
                through: None,
                target_foreign_key: None,
            }],
            kind: TableKind::ReadModel,
        };
        let child = TableSchema {
            model_name: "ChildView".into(),
            table_name: "children".into(),
            columns: vec![
                TableColumn {
                    primary_key: true,
                    ..TableColumn::new("id", "id", ColumnType::Text)
                },
                TableColumn::new("parent_id", "parent_id", ColumnType::Text),
            ],
            primary_key: PrimaryKey::new(["id"]),
            version_column: None,
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships: Vec::new(),
            kind: TableKind::ReadModel,
        };
        let both = build_surface(&[parent.clone(), child], &SurfaceOptions::sqlite()).unwrap();
        assert!(both
            .models
            .get("ParentView")
            .unwrap()
            .relationships
            .iter()
            .any(|r| r.name == "children"));

        let parent_only = build_surface(&[parent], &SurfaceOptions::sqlite()).unwrap();
        assert!(parent_only
            .models
            .get("ParentView")
            .unwrap()
            .relationships
            .is_empty());
    }

    #[test]
    fn surface_rejects_relationship_and_generated_aggregate_field_collisions() {
        let child = TableSchema {
            model_name: "CollisionChild".into(),
            table_name: "collision_children".into(),
            columns: vec![
                TableColumn {
                    primary_key: true,
                    ..TableColumn::new("id", "id", ColumnType::Text)
                },
                TableColumn::new("parent_id", "parent_id", ColumnType::Text),
            ],
            primary_key: PrimaryKey::new(["id"]),
            version_column: None,
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships: Vec::new(),
            kind: TableKind::ReadModel,
        };
        let parent = TableSchema {
            model_name: "CollisionParent".into(),
            table_name: "collision_parents".into(),
            columns: vec![
                TableColumn {
                    primary_key: true,
                    ..TableColumn::new("id", "id", ColumnType::Text)
                },
                TableColumn::new("children_aggregate", "children_aggregate", ColumnType::Text),
            ],
            primary_key: PrimaryKey::new(["id"]),
            version_column: None,
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships: vec![RelationshipDef {
                field_name: "children".into(),
                kind: RelationshipKind::HasMany,
                target_model: "CollisionChild".into(),
                foreign_key: Some("parent_id".into()),
                through: None,
                target_foreign_key: None,
            }],
            kind: TableKind::ReadModel,
        };
        let error = build_surface(&[parent, child], &SurfaceOptions::sqlite()).unwrap_err();
        assert!(error.contains("relationship aggregate `children_aggregate` collides"));
    }

    #[test]
    fn relationship_keys_canonicalize_rust_field_names_to_graphql_columns() {
        let account = TableSchema {
            model_name: "AccountView".into(),
            table_name: "accounts".into(),
            columns: vec![TableColumn {
                primary_key: true,
                ..TableColumn::new("account_id", "account_id", ColumnType::Text)
            }],
            primary_key: PrimaryKey::new(["account_id"]),
            version_column: None,
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships: Vec::new(),
            kind: TableKind::ReadModel,
        };
        let order = TableSchema {
            model_name: "RenamedOrderView".into(),
            table_name: "renamed_orders".into(),
            columns: vec![
                TableColumn {
                    primary_key: true,
                    ..TableColumn::new("order_id", "order_id", ColumnType::Text)
                },
                TableColumn::new("accountId", "account_id", ColumnType::Text),
            ],
            primary_key: PrimaryKey::new(["order_id"]),
            version_column: None,
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships: vec![RelationshipDef {
                field_name: "account".into(),
                kind: RelationshipKind::BelongsTo,
                target_model: "AccountView".into(),
                foreign_key: Some("accountId".into()),
                through: None,
                target_foreign_key: None,
            }],
            kind: TableKind::ReadModel,
        };
        let surface = build_surface(&[order, account], &SurfaceOptions::sqlite()).unwrap();
        let relationship = &surface.models["RenamedOrderView"].relationships[0];
        assert_eq!(
            relationship.keys,
            SurfaceRelationshipKeys::Direct {
                local: vec!["account_id".into()],
                remote: vec!["account_id".into()],
            }
        );
    }

    #[test]
    fn pool_free_role_selection_rejects_only_reachable_composite_relationships() {
        let composite = TableSchema {
            model_name: "CompositeView".into(),
            table_name: "composites".into(),
            columns: vec![
                TableColumn {
                    primary_key: true,
                    ..TableColumn::new("tenant_id", "tenant_id", ColumnType::Text)
                },
                TableColumn {
                    primary_key: true,
                    ..TableColumn::new("record_id", "record_id", ColumnType::Text)
                },
            ],
            primary_key: PrimaryKey::new(["tenant_id", "record_id"]),
            version_column: None,
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships: Vec::new(),
            kind: TableKind::ReadModel,
        };
        let simple = TableSchema {
            model_name: "SimpleView".into(),
            table_name: "simples".into(),
            columns: vec![
                TableColumn {
                    primary_key: true,
                    ..TableColumn::new("simple_id", "simple_id", ColumnType::Text)
                },
                TableColumn::new("tenant_id", "tenant_id", ColumnType::Text),
            ],
            primary_key: PrimaryKey::new(["simple_id"]),
            version_column: None,
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships: vec![RelationshipDef {
                field_name: "composite".into(),
                kind: RelationshipKind::BelongsTo,
                target_model: "CompositeView".into(),
                foreign_key: Some("tenant_id".into()),
                through: None,
                target_foreign_key: None,
            }],
            kind: TableKind::ReadModel,
        };
        let full = build_surface(&[simple, composite], &SurfaceOptions::sqlite()).unwrap();

        // Hidden catalog metadata cannot create a false rejection.
        let source_only = surface_for_role(
            &full,
            "source-only",
            &BTreeMap::from([("SimpleView".into(), RoleGrant::all_columns())]),
        )
        .unwrap();
        assert!(source_only.models["SimpleView"].relationships.is_empty());

        // A server-only policy may legitimately traverse a denied model, but
        // it must still be rejected when the runtime's current join compiler
        // cannot represent that composite identity safely.
        let hidden_policy_error = surface_for_role(
            &full,
            "source-policy",
            &BTreeMap::from([(
                "SimpleView".into(),
                RoleGrant::all_columns().rows(super::super::rel(
                    "composite",
                    super::super::col("tenant_id").eq("tenant-a"),
                )),
            )]),
        )
        .unwrap_err();
        assert!(
            hidden_policy_error.contains("composite-key topology"),
            "{hidden_policy_error}"
        );

        // Once both models are reachable, pool-free export fails at the same
        // selected-Surface boundary as runtime engine construction.
        let error = surface_for_role(
            &full,
            "both",
            &BTreeMap::from([
                ("CompositeView".into(), RoleGrant::all_columns()),
                ("SimpleView".into(), RoleGrant::all_columns()),
            ]),
        )
        .unwrap_err();
        assert!(error.contains("relationship topology"), "{error}");
    }

    /// Production path: build_surface → surface_for_role → SDL (gap A10).
    #[test]
    fn role_sdl_production_path_omits_ungranted_columns() {
        use super::super::sdl::{graphql_sdl_for_role, SdlOptions};

        let mut grants = BTreeMap::new();
        grants.insert(
            "OrderView".to_string(),
            RoleGrant::columns(["order_id", "status"]),
        );
        let sdl = graphql_sdl_for_role(&[orders()], &SdlOptions::sqlite(), "user", &grants)
            .expect("role sdl");

        // Granted
        assert!(
            sdl.contains("order_id") && sdl.contains("status"),
            "expected granted columns in SDL: {sdl}"
        );
        // Ungranted column fields must not appear on the object type body.
        // (meta / customer_id were not granted)
        assert!(
            !sdl.contains("customer_id"),
            "ungranted customer_id leaked into role SDL: {sdl}"
        );
        assert!(
            !sdl.contains("meta"),
            "ungranted meta leaked into role SDL: {sdl}"
        );
        // SQLite: no PG JSON ops even if JSON columns were granted
        for forbidden in ["_contains", "_contained_in", "_has_key"] {
            assert!(
                !sdl.contains(forbidden),
                "SQLite role SDL must not expose {forbidden}"
            );
        }
    }

    #[test]
    fn role_sdl_empty_grants_has_no_query_roots() {
        use super::super::sdl::{graphql_sdl_for_role, SdlOptions};

        let sdl =
            graphql_sdl_for_role(&[orders()], &SdlOptions::sqlite(), "anon", &BTreeMap::new())
                .expect("empty role sdl");
        // No list roots for orders when model ungranted
        assert!(
            !sdl.contains("orders(") && !sdl.contains("orders:"),
            "empty grants should not expose orders roots: {sdl}"
        );
        assert!(sdl.contains("type Query {\n  _empty: Boolean!\n}"));
    }

    #[test]
    fn constant_validation_uses_exact_wire_scalar_domains() {
        let command = SurfaceCommand {
            command_name: "test.constant".into(),
            field_name: "test_constant".into(),
            roles: Vec::new(),
            input: SurfaceCommandShape::None,
            output: SurfaceCommandShape::Typed(SurfaceTypeDef {
                name: "ConstantPayload".into(),
                fields: vec![SurfaceTypeField {
                    name: "ok".into(),
                    type_name: "Boolean".into(),
                    nullable: false,
                    list: false,
                    item_nullable: false,
                    nested: None,
                }],
            }),
            consistency: CommandConsistency::Accepted,
            input_defaults: Vec::new(),
            effects: Some(CommandEffects::revalidate()),
            confirmations: Vec::new(),
            projected_model: None,
            direct_projection: None,
            confirmation_unavailable: false,
        };
        let constant = |value| EffectExpression::Constant { value };
        let column = |name: &str, scalar: &str| ColumnField {
            name: name.into(),
            scalar: scalar.into(),
            nullable: false,
        };

        for (value, scalar) in [
            (serde_json::json!(1.5), "BigInt"),
            (serde_json::json!(2_147_483_648_i64), "Int"),
            (serde_json::json!("not-a-timestamp"), "Timestamptz"),
            (serde_json::json!("***"), "Bytea"),
        ] {
            assert!(validate_effect_expression(
                &command,
                &constant(value),
                &column("value", scalar),
            )
            .is_err());
        }

        for (value, scalar) in [
            (serde_json::json!(42), "BigInt"),
            (serde_json::json!("2026-07-22T12:30:45.123Z"), "Timestamptz"),
            (serde_json::json!("AQID"), "Bytea"),
        ] {
            assert!(validate_effect_expression(
                &command,
                &constant(value),
                &column("value", scalar),
            )
            .is_ok());
        }
    }

    #[test]
    fn missing_surface_primary_key_column_is_a_configuration_error_not_a_panic() {
        let surface = build_surface(&[orders()], &SurfaceOptions::sqlite()).unwrap();
        let mut model = surface.models["OrderView"].clone();
        model.columns.retain(|column| column.name != "order_id");
        let models = BTreeMap::from([("OrderView".into(), model)]);
        let command = SurfaceCommand {
            command_name: "order.patch".into(),
            field_name: "order_patch".into(),
            roles: Vec::new(),
            input: SurfaceCommandShape::None,
            output: SurfaceCommandShape::Typed(SurfaceTypeDef {
                name: "PatchOrderPayload".into(),
                fields: vec![SurfaceTypeField {
                    name: "order_id".into(),
                    type_name: "String".into(),
                    nullable: false,
                    list: false,
                    item_nullable: false,
                    nested: None,
                }],
            }),
            consistency: CommandConsistency::Accepted,
            input_defaults: Vec::new(),
            effects: Some(CommandEffects::revalidate()),
            confirmations: Vec::new(),
            projected_model: None,
            direct_projection: None,
            confirmation_unavailable: false,
        };
        let key = EffectKey {
            fields: vec![EffectFieldValue {
                field: "order_id".into(),
                value: EffectExpression::Constant {
                    value: serde_json::json!("order-1"),
                },
            }],
        };

        let result =
            std::panic::catch_unwind(|| validate_effect_key(&models, &command, "OrderView", &key));
        let error = result
            .expect("malformed Surface metadata must not panic")
            .expect_err("missing primary-key column must fail closed");
        assert!(error.contains("missing or hidden on the selected Surface"));
    }

    /// A7: role×dialect inventory + IR→SDL ops stay aligned (portable fixture).
    #[test]
    fn a7_role_dialect_parity_inventory_and_sdl_ops() {
        use super::super::sdl::{graphql_sdl_for_role, SdlOptions};

        let full_sqlite = build_surface(&[orders()], &SurfaceOptions::sqlite()).unwrap();
        let full_pg = build_surface(&[orders()], &SurfaceOptions::postgres()).unwrap();

        // Dialect honesty on full surface
        let sqlite_json = full_sqlite.comparison_ops_for_scalar("JSON");
        let pg_json = full_pg.comparison_ops_for_scalar("JSON");
        assert!(sqlite_json.contains(&"_eq"));
        assert!(!sqlite_json.contains(&"_contains"));
        assert!(pg_json.contains(&"_contains"));

        let mut grants = BTreeMap::new();
        grants.insert(
            "OrderView".to_string(),
            RoleGrant::columns(["order_id", "status"]),
        );

        for (opts, dialect_label) in [
            (SdlOptions::sqlite(), "sqlite"),
            (SdlOptions::postgres(), "postgres"),
        ] {
            let full = build_surface(
                &[orders()],
                &SurfaceOptions {
                    dialect: if opts.jsonb_operators {
                        SurfaceDialect::Postgres
                    } else {
                        SurfaceDialect::Sqlite
                    },
                    aggregates: opts.aggregates,
                    subscriptions: opts.subscriptions,
                    default_limit: 100,
                    max_limit: 1000,
                },
            )
            .unwrap();
            let role_s = surface_for_role(&full, "user", &grants).unwrap();
            let roots: Vec<_> = role_s.query_root_names();
            assert!(
                roots.contains(&"orders") && roots.contains(&"orders_by_pk"),
                "{dialect_label}: missing list/by_pk roots {roots:?}"
            );
            assert!(
                !roots.iter().any(|n| n.contains("aggregate")),
                "{dialect_label}: aggregate without grant"
            );
            let cols: Vec<_> = role_s
                .models
                .get("OrderView")
                .unwrap()
                .columns
                .iter()
                .map(|c| c.name.as_str())
                .collect();
            assert_eq!(cols, vec!["order_id", "status"]);

            let sdl = graphql_sdl_for_role(&[orders()], &opts, "user", &grants).unwrap();
            assert!(
                sdl.contains("order_id") && !sdl.contains("customer_id"),
                "{dialect_label}: SDL column leak: {sdl}"
            );
            // SQLite role SDL never exposes PG JSON ops even if dialect flag wrong on unused scalars
            if !opts.jsonb_operators {
                for forbidden in ["_contains", "_contained_in", "_has_key"] {
                    assert!(
                        !sdl.contains(forbidden),
                        "{dialect_label}: {forbidden} in SDL"
                    );
                }
            }
        }
    }
}
