//! Versioned client contract derived from the shared, role-filtered GraphQL
//! [`Surface`](super::surface::Surface).
//!
//! This module is intentionally pool-free. Runtime schema construction, engine
//! export, and `dctl` all hand the same Surface to
//! [`DistributedClientSurfaceExport::manifest`]; no consumer re-walks a table,
//! command, permission, relationship, or projector registry.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::command_contract::{CommandConsistency, CommandEffectFallback};
use super::complexity_contract::{default_weights, DEFAULT_MAX_COMPLEXITY, DEFAULT_MAX_DEPTH};
use super::filter::FilterExpr;
use super::naming::{aggregate_fields_type_name, aggregate_type_name};
use super::surface::{
    model_has_client_normalized_identity, RootKind, Surface, SurfaceArgument, SurfaceArgumentKind,
    SurfaceCommand, SurfaceCommandShape, SurfaceRelationshipKeys, SurfaceRowPolicy,
    SurfaceSelection, SurfaceTypeDef,
};
use crate::manifest::DistributedProjectManifest;
use crate::table::RelationshipKind;

pub const DISTRIBUTED_CLIENT_MANIFEST_VERSION: u32 = 6;
pub const DISTRIBUTED_CLIENT_PROTOCOL_VERSION: u32 = 2;
// Manifest v6 extends only compiler-owned query metadata. Keep the causal
// protocol fingerprint on the manifest epoch that introduced protocol v2 so
// receipt/status/live envelopes do not become falsely incompatible.
const DISTRIBUTED_CLIENT_PROTOCOL_MANIFEST_EPOCH: u32 = 4;
const COMMAND_EXTENSION_SLOTS_VERSION: u32 = 2;
const COMMAND_CONFIRMATIONS_VERSION: u32 = 1;
const PROJECTOR_ENTRY_VERSION: u32 = 1;
const PROTOCOL_OPERATIONS_VERSION: u32 = 1;
const QUERY_CAPABILITIES_VERSION: u32 = 1;
const QUERY_COMPLEXITY_VERSION: u32 = 1;
const KEY_ENCODING: &str = "canonical_json_tuple_v1";
const DEFAULT_MAX_BOOL_WIDTH: u64 = 256;
const DEFAULT_MAX_IN_LIST: u64 = 1_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientManifestError(pub String);

impl std::fmt::Display for ClientManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ClientManifestError {}

impl From<serde_json::Error> for ClientManifestError {
    fn from(error: serde_json::Error) -> Self {
        Self(format!("client manifest serialization failed: {error}"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClientSurfaceIdentity {
    Role { name: String },
    Application { name: String, roles: Vec<String> },
}

impl ClientSurfaceIdentity {
    pub fn role(name: impl Into<String>) -> Self {
        Self::Role { name: name.into() }
    }

    pub fn application(
        name: impl Into<String>,
        roles: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        let mut roles: Vec<String> = roles.into_iter().map(Into::into).collect();
        roles.sort();
        roles.dedup();
        Self::Application {
            name: name.into(),
            roles,
        }
    }

    fn canonicalized(self) -> Result<Self, ClientManifestError> {
        match self {
            Self::Role { name } if name.trim().is_empty() => Err(ClientManifestError(
                "role surface name must not be empty".into(),
            )),
            Self::Role { name } => Ok(Self::Role { name }),
            Self::Application { name, .. } if name.trim().is_empty() => Err(ClientManifestError(
                "application surface name must not be empty".into(),
            )),
            Self::Application { name, mut roles } => {
                if roles.iter().any(|role| role.trim().is_empty()) {
                    return Err(ClientManifestError(format!(
                        "application surface `{name}` contains an empty role id"
                    )));
                }
                roles.sort();
                roles.dedup();
                if roles.is_empty() {
                    return Err(ClientManifestError(format!(
                        "application surface `{name}` must declare at least one role"
                    )));
                }
                Ok(Self::Application { name, roles })
            }
        }
    }
}

/// The one portable export object shared by engine and CLI harnesses.
#[derive(Clone)]
pub struct DistributedClientSurfaceExport {
    service_id: String,
    identity: ClientSurfaceIdentity,
    surface: Arc<Surface>,
    execution: ClientExecutionLimits,
}

/// Do not transitively format the selected Surface: it retains a private full
/// catalog solely for server-side policy validation.
impl std::fmt::Debug for DistributedClientSurfaceExport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DistributedClientSurfaceExport")
            .field("service_id", &self.service_id)
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

impl DistributedClientSurfaceExport {
    fn new(
        service_id: impl Into<String>,
        identity: ClientSurfaceIdentity,
        surface: impl Into<Arc<Surface>>,
        execution: ClientExecutionLimits,
    ) -> Self {
        Self {
            service_id: service_id.into(),
            identity,
            surface: surface.into(),
            execution,
        }
    }

    /// Safe, low-boilerplate export path: authorization identity is derived
    /// from the selected Surface and cannot be caller-asserted.
    pub(crate) fn from_selected(
        service_id: impl Into<String>,
        surface: impl Into<Arc<Surface>>,
    ) -> Result<Self, ClientManifestError> {
        Self::from_selected_with_execution(service_id, surface, ClientExecutionLimits::default())
    }

    pub(crate) fn from_selected_with_execution(
        service_id: impl Into<String>,
        surface: impl Into<Arc<Surface>>,
        execution: ClientExecutionLimits,
    ) -> Result<Self, ClientManifestError> {
        let surface = surface.into();
        let service_id = service_id.into();
        let identity = match &surface.selection {
            SurfaceSelection::Catalog => {
                return Err(ClientManifestError(
                    "client exports require an explicitly role- or application-selected Surface"
                        .into(),
                ));
            }
            SurfaceSelection::Role { name } => ClientSurfaceIdentity::role(name),
            SurfaceSelection::Application { name, roles } => {
                ClientSurfaceIdentity::application(name, roles.clone())
            }
        };
        validate_service_provenance(&service_id, &surface)?;
        Ok(Self::new(service_id, identity, surface, execution))
    }

    /// Build a portable export whose service identity comes from the same
    /// project manifest that supplied its table inventory.
    pub fn from_project(
        project: &DistributedProjectManifest,
        surface: impl Into<Arc<Surface>>,
    ) -> Result<Self, ClientManifestError> {
        let surface = surface.into();
        for model in surface.models.values() {
            let Some(original) = project.tables.iter().find(|schema| {
                schema.model_name == model.model_name && schema.table_name == model.table_name
            }) else {
                return Err(ClientManifestError(format!(
                    "selected Surface model `{}` does not match the supplied project manifest inventory",
                    model.model_name
                )));
            };
            let mut selected_schema = model.schema.clone();
            for column in &mut selected_schema.columns {
                if let Some(original_column) = original
                    .columns
                    .iter()
                    .find(|candidate| candidate.column_name == column.column_name)
                {
                    column.skipped = original_column.skipped;
                }
            }
            selected_schema.relationships = original.relationships.clone();
            if &selected_schema != original {
                return Err(ClientManifestError(format!(
                    "selected Surface model `{}` does not match the supplied project manifest inventory",
                    model.model_name
                )));
            }
        }
        Self::from_selected(project.name.clone(), surface)
    }

    pub fn manifest(&self) -> Result<DistributedClientManifest, ClientManifestError> {
        client_manifest_from_surface_with_execution(
            &self.service_id,
            self.identity.clone(),
            &self.surface,
            self.execution.clone(),
        )
    }

    pub fn service_id(&self) -> &str {
        &self.service_id
    }

    pub fn identity(&self) -> &ClientSurfaceIdentity {
        &self.identity
    }

    pub fn surface(&self) -> &Arc<Surface> {
        &self.surface
    }

    pub fn manifest_json_pretty(&self) -> Result<String, ClientManifestError> {
        Ok(serde_json::to_string_pretty(&self.manifest()?)?)
    }
}

fn validate_service_provenance(
    service_id: &str,
    surface: &Surface,
) -> Result<(), ClientManifestError> {
    let has_typed_commands = surface
        .commands
        .iter()
        .any(|command| command.consistency.is_some());
    match (&surface.service_binding, has_typed_commands) {
        (Some(binding), _) if binding.service_id != service_id => Err(ClientManifestError(
            format!(
                "client export service ID `{service_id}` does not match typed Surface provenance `{}`",
                binding.service_id
            ),
        )),
        (None, true) => Err(ClientManifestError(
            "typed client export requires Surface provenance from Surface::with_service"
                .into(),
        )),
        _ => Ok(()),
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DistributedClientManifest {
    pub manifest_version: u32,
    pub protocol_version: u32,
    pub service_id: String,
    pub surface: ClientSurfaceIdentity,
    pub schema_fingerprint: String,
    pub protocol_fingerprint: String,
    pub execution: ClientExecutionLimits,
    pub capabilities: ClientCapabilities,
    pub scalar_codecs: Vec<ScalarCodec>,
    pub models: Vec<ClientModel>,
    pub roots: Vec<ClientRoot>,
    pub commands: Vec<ClientCommand>,
    pub protocol_operations: ClientProtocolOperations,
    pub projectors: Vec<ClientProjector>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientExecutionLimits {
    pub max_depth: u64,
    pub max_complexity: u64,
    pub max_bool_width: u64,
    pub max_in_list: u64,
    pub complexity: ClientComplexityWeights,
}

impl Default for ClientExecutionLimits {
    fn default() -> Self {
        Self {
            max_depth: DEFAULT_MAX_DEPTH as u64,
            max_complexity: DEFAULT_MAX_COMPLEXITY as u64,
            max_bool_width: DEFAULT_MAX_BOOL_WIDTH,
            max_in_list: DEFAULT_MAX_IN_LIST,
            complexity: ClientComplexityWeights::current(),
        }
    }
}

impl ClientExecutionLimits {
    pub(crate) fn from_runtime(
        max_depth: usize,
        max_complexity: usize,
        max_bool_width: usize,
        max_in_list: usize,
    ) -> Result<Self, ClientManifestError> {
        Ok(Self {
            max_depth: u64::try_from(max_depth).map_err(|_| {
                ClientManifestError("GraphQL max_depth exceeds the client manifest range".into())
            })?,
            max_complexity: u64::try_from(max_complexity).map_err(|_| {
                ClientManifestError(
                    "GraphQL max_complexity exceeds the client manifest range".into(),
                )
            })?,
            max_bool_width: u64::try_from(max_bool_width).map_err(|_| {
                ClientManifestError(
                    "GraphQL max_bool_width exceeds the client manifest range".into(),
                )
            })?,
            max_in_list: u64::try_from(max_in_list).map_err(|_| {
                ClientManifestError("GraphQL max_in_list exceeds the client manifest range".into())
            })?,
            complexity: ClientComplexityWeights::current(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientComplexityWeights {
    pub version: u32,
    pub scalar: u64,
    pub belongs_to: u64,
    pub has_many: u64,
    pub m2m: u64,
    pub aggregate: u64,
    pub list_root: u64,
    pub by_pk: u64,
    pub list_fanout: u64,
}

impl ClientComplexityWeights {
    fn current() -> Self {
        let weights = default_weights();
        Self {
            version: QUERY_COMPLEXITY_VERSION,
            scalar: weights.scalar as u64,
            belongs_to: weights.belongs_to as u64,
            has_many: weights.has_many as u64,
            m2m: weights.m2m as u64,
            aggregate: weights.aggregate as u64,
            list_root: weights.list_root as u64,
            by_pk: weights.by_pk as u64,
            list_fanout: weights.list_fanout as u64,
        }
    }
}

/// Framework-owned operations generated alongside application operations.
///
/// Keeping these documents in the manifest means clients never synthesize a
/// status query or guess the server's protocol field selection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientProtocolOperations {
    pub version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_status: Option<ClientProtocolOperation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientProtocolOperation {
    pub name: String,
    pub operation: String,
    pub operation_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientCapabilities {
    pub live_queries: bool,
    pub record_revisions: bool,
    pub tombstones: bool,
    pub causal_receipts: bool,
    pub live_resume: bool,
    /// Safe behavior whenever exact query evidence or resume is unavailable.
    pub query_fallback: String,
    pub cache_scope: bool,
    /// Durable restore of confirmed normalized state (task 11).
    pub confirmed_persistence: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScalarCodec {
    pub scalar: String,
    pub codec: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClientModel {
    pub id: String,
    pub typename: String,
    pub source_table: String,
    pub dependencies: Vec<String>,
    pub normalization: ModelNormalization,
    pub fields: Vec<ClientField>,
    pub relationships: Vec<ClientRelationship>,
    /// Exact GraphQL predicate input owned by this role-selected model.
    ///
    /// This is deliberately model-level rather than copied from a list root or
    /// relationship selection. GraphQL bool-exp relationships exist regardless
    /// of whether the corresponding object field accepts list arguments.
    pub filter_input: ClientFilterInput,
    pub row_policy: ClientRowPolicy,
    pub record_revisions: bool,
    pub tombstones: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModelNormalization {
    Normalized {
        fields: Vec<ClientKeyField>,
        encoding: String,
    },
    Embedded,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientKeyField {
    pub name: String,
    pub codec: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientField {
    pub name: String,
    pub scalar: String,
    pub codec: String,
    pub nullable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientFilterInput {
    pub type_name: String,
    pub fields: Vec<ClientFilterField>,
    pub relationships: Vec<ClientFilterInputRelationship>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientFilterInputRelationship {
    pub field: String,
    pub target_type: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClientRowPolicy {
    Unrestricted,
    Predicate { expression: FilterExpr },
    ServerOnly,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClientRelationship {
    pub name: String,
    pub target_model: String,
    pub target_typename: String,
    pub kind: ClientRelationshipKind,
    pub list: bool,
    /// Copied from the role-filtered Surface. Lists are non-null collections;
    /// singular relationships retain the authoritative object nullability.
    pub nullable: bool,
    pub arguments: Vec<ClientArgument>,
    pub key_mapping: RelationshipKeyMapping,
    pub maintenance: ClientRelationshipMaintenance,
    pub dependencies: Vec<String>,
    pub filter: Option<ClientFilterSemantics>,
    pub order: Option<ClientOrderSemantics>,
    pub pagination: Option<ClientPaginationSemantics>,
    pub aggregate: Option<ClientRelationshipAggregate>,
    pub live: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientRelationshipAggregate {
    pub name: String,
    pub arguments: Vec<ClientArgument>,
    pub semantics: ClientAggregateSemantics,
    pub dependencies: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientRelationshipKind {
    HasMany,
    BelongsTo,
    ManyToMany,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RelationshipKeyMapping {
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
    ThroughOpaque {
        local: Vec<String>,
        remote: Vec<String>,
        dependency: String,
    },
    Embedded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientRelationshipMaintenance {
    /// Incoming normalized entities are sufficient to update membership.
    Local,
    /// Dependency changes mark the relationship stale and trigger revalidation.
    Revalidate,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClientRoot {
    pub id: String,
    pub operation: ClientRootOperation,
    pub name: String,
    pub kind: ClientRootKind,
    pub model: String,
    pub arguments: Vec<ClientArgument>,
    pub filter: Option<ClientFilterSemantics>,
    pub order: Option<ClientOrderSemantics>,
    pub pagination: Option<ClientPaginationSemantics>,
    pub aggregate: Option<ClientAggregateSemantics>,
    pub dependencies: Vec<String>,
    pub live: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientRootOperation {
    Query,
    Subscription,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientRootKind {
    List,
    ByPk,
    Aggregate,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientArgument {
    pub name: String,
    pub kind: ClientArgumentKind,
    pub type_name: String,
    pub nullable: bool,
    pub list: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientArgumentKind {
    Filter,
    Order,
    Limit,
    Offset,
    PrimaryKey,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClientFilterSemantics {
    pub fields: Vec<ClientFilterField>,
    pub relationships: Vec<String>,
    pub row_policy: ClientRowPolicy,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientFilterField {
    pub name: String,
    pub operators: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientOrderSemantics {
    pub fields: Vec<String>,
    pub values: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientPaginationSemantics {
    pub kind: String,
    pub default_limit: u64,
    pub max_limit: u64,
    pub coverage: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientAggregateSemantics {
    /// Exact GraphQL wrapper object selected by this root or relationship.
    pub wrapper_typename: String,
    /// Exact GraphQL object returned by the wrapper's `aggregate` field.
    pub fields_typename: String,
    /// Authoritative bounded-window semantics for the wrapper's `nodes` field.
    ///
    /// Aggregate roots do not expose the ordinary list root's pagination
    /// metadata, and relationship aggregates are distinct fields from their
    /// sibling list relationships. Carrying this plan on the aggregate itself
    /// prevents clients from treating an omitted/default-limited `nodes`
    /// selection as a complete model collection.
    pub nodes_pagination: ClientPaginationSemantics,
    pub count: bool,
    pub nodes: bool,
    pub sum: Vec<String>,
    pub avg: Vec<String>,
    pub min: Vec<String>,
    pub max: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientCommand {
    pub version: u32,
    pub name: String,
    pub mutation_field: String,
    pub grants: Vec<String>,
    pub input: ClientCommandShape,
    pub output: ClientCommandShape,
    /// Canonical executable GraphQL operation; clients never synthesize it.
    pub operation: String,
    pub operation_hash: String,
    pub extensions: ClientCommandExtensionSlots,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClientCommandShape {
    None,
    Json { codec: String },
    Object { definition: ClientTypeDef },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientTypeDef {
    pub name: String,
    pub fields: Vec<ClientTypeField>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientTypeField {
    pub name: String,
    pub type_name: String,
    pub nullable: bool,
    pub list: bool,
    pub item_nullable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nested: Option<Box<ClientTypeDef>>,
}

/// Versioned slots intentionally left empty by task 3. Task 4 owns their typed
/// declaration and population; absence means no guessed client semantics.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientCommandExtensionSlots {
    pub version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consistency: Option<CommandConsistencyExtension>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direct_projection: Option<CommandDirectProjectionExtension>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_defaults: Option<CommandInputDefaultsExtension>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effects: Option<CommandEffectsExtension>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmations: Option<CommandConfirmationsExtension>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandConsistencyExtension {
    pub version: u32,
    pub kind: String,
}

/// Opaque same-transaction target for one `Projected<T>` command.
///
/// The topology digest binds the scope-codec version, accepted facts, complete
/// owned schemas, partition declaration, and physical ownership on the server.
/// The role-selected client contract therefore needs only this exact identity,
/// never the hidden topology inventory used to compile it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandDirectProjectionExtension {
    pub topology: ClientProjectionTopologyIdentity,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partition: Option<serde_json::Value>,
    pub change_epoch: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientProjectionTopologyIdentity {
    pub version: u32,
    pub name: String,
    pub digest: String,
}

/// Generators applied once to the canonical command input before dispatch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandInputDefaultsExtension {
    pub version: u32,
    pub defaults: Vec<serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandEffectsExtension {
    pub version: u32,
    pub operations: Vec<serde_json::Value>,
    pub fallback: String,
}

/// Declaration-owned projector progress expected after a fact commit.
/// Entries use the same closed input-expression/key IR as optimistic effects.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandConfirmationsExtension {
    pub version: u32,
    /// `finite` contains the complete authorized edge set; `unavailable`
    /// intentionally carries no topology and requires revalidation.
    pub kind: String,
    pub expected: Vec<serde_json::Value>,
    pub fallback: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientProjector {
    pub version: u32,
    pub name: String,
    pub facts: Vec<String>,
    pub models: Vec<String>,
    pub dependencies: Vec<String>,
    pub causal_confirmation: bool,
}

fn model_has_visible_causal_owner(surface: &Surface, model: &str) -> bool {
    surface
        .projectors
        .iter()
        .filter(|projector| projector.models.iter().any(|candidate| candidate == model))
        .take(2)
        .count()
        == 1
}

fn selected_query_models(surface: &Surface) -> BTreeSet<String> {
    let mut selected = surface
        .query_fields
        .iter()
        .map(|root| root.model_name.clone())
        .collect::<BTreeSet<_>>();
    let mut pending = selected.iter().cloned().collect::<Vec<_>>();
    while let Some(model_name) = pending.pop() {
        let Some(model) = surface.models.get(&model_name) else {
            continue;
        };
        for relationship in &model.relationships {
            if selected.insert(relationship.target_model.clone()) {
                pending.push(relationship.target_model.clone());
            }
        }
    }
    selected
}

fn selected_query_dependencies(surface: &Surface) -> BTreeSet<String> {
    let mut dependencies = surface
        .query_fields
        .iter()
        .flat_map(|root| root.dependencies.iter().cloned())
        .collect::<BTreeSet<_>>();
    for model_name in selected_query_models(surface) {
        let Some(model) = surface.models.get(&model_name) else {
            continue;
        };
        dependencies.insert(model.table_name.clone());
        for relationship in &model.relationships {
            dependencies.extend(relationship.dependencies.iter().cloned());
            if let Some(aggregate) = &relationship.aggregate {
                dependencies.extend(aggregate.dependencies.iter().cloned());
            }
        }
    }
    dependencies
}

fn query_footprint_has_record_evidence(surface: &Surface) -> bool {
    let models = selected_query_models(surface);
    !models.is_empty()
        && models
            .iter()
            .all(|model| model_has_visible_causal_owner(surface, model))
}

fn projector_partition_matches_authorization(
    surface: &Surface,
    projector: &super::surface::SurfaceProjector,
) -> bool {
    projector.models.iter().all(|model| {
        surface
            .models
            .get(model)
            .is_some_and(|model| matches!(model.row_policy, SurfaceRowPolicy::Unrestricted))
    })
}

fn query_footprint_supports_live_resume(surface: &Surface) -> bool {
    if surface.subscription_fields.is_empty() {
        return false;
    }
    let dependencies = selected_query_dependencies(surface);
    if dependencies.is_empty() {
        return false;
    }
    dependencies.iter().all(|dependency| {
        let mut owners = surface.projectors.iter().filter(|projector| {
            projector
                .dependencies
                .iter()
                .any(|candidate| candidate == dependency)
        });
        let Some(owner) = owners.next() else {
            return false;
        };
        owners.next().is_none()
            && owner.partition.preserves_source_sequence()
            && owner.change_epoch.is_some()
            && projector_partition_matches_authorization(surface, owner)
    })
}

fn command_direct_projection_extension(
    command: &SurfaceCommand,
    surface: &Surface,
) -> Result<Option<CommandDirectProjectionExtension>, ClientManifestError> {
    let projected = matches!(command.consistency, Some(CommandConsistency::Projected));
    let Some(target) = command.direct_projection.as_ref() else {
        return if projected {
            Err(ClientManifestError(format!(
                "typed projected command `{}` is missing its bound direct projection target",
                command.command_name
            )))
        } else {
            Ok(None)
        };
    };
    if !projected {
        return Err(ClientManifestError(format!(
            "typed non-projected command `{}` cannot export a direct projection target",
            command.command_name
        )));
    }
    let retained = command.projected_model.as_ref().ok_or_else(|| {
        ClientManifestError(format!(
            "typed projected command `{}` is missing its retained relational model",
            command.command_name
        ))
    })?;
    if target.model != retained.model {
        return Err(ClientManifestError(format!(
            "typed projected command `{}` direct target model `{}` differs from retained model `{}`",
            command.command_name, target.model, retained.model
        )));
    }
    let model = surface.models.get(&target.model).ok_or_else(|| {
        ClientManifestError(format!(
            "typed projected command `{}` direct target model `{}` is not authorized on this client surface",
            command.command_name, target.model
        ))
    })?;
    if !model_has_client_normalized_identity(model) {
        return Err(ClientManifestError(format!(
            "typed projected command `{}` direct target model `{}` has no complete authorized client identity",
            command.command_name, target.model
        )));
    }
    let SurfaceCommandShape::Typed(output) = &command.output else {
        return Err(ClientManifestError(format!(
            "typed projected command `{}` must return its exact relational model object",
            command.command_name
        )));
    };
    if output.name != model.object_name || output.fields.len() != model.columns.len() {
        return Err(ClientManifestError(format!(
            "typed projected command `{}` output does not match authorized model `{}`",
            command.command_name, target.model
        )));
    }
    for field in &output.fields {
        let matches = model.columns.iter().any(|column| {
            field.name == column.name
                && field.type_name == column.scalar
                && field.nullable == column.nullable
                && !field.list
                && !field.item_nullable
                && field.nested.is_none()
        });
        if !matches {
            return Err(ClientManifestError(format!(
                "typed projected command `{}` output field `{}.{}` differs from authorized model `{}`",
                command.command_name, output.name, field.name, target.model
            )));
        }
    }

    let topology = target.protocol_topology().ok_or_else(|| {
        ClientManifestError(format!(
            "typed projected command `{}` direct target is not bound to its compiled protocol topology",
            command.command_name
        ))
    })?;
    if topology.name() != target.projector {
        return Err(ClientManifestError(format!(
            "typed projected command `{}` direct target projector `{}` differs from bound topology `{}`",
            command.command_name,
            target.projector,
            topology.name()
        )));
    }

    let visible_owners = surface
        .projectors
        .iter()
        .filter(|projector| projector.models.iter().any(|model| model == &target.model))
        .collect::<Vec<_>>();
    match visible_owners.as_slice() {
        [] => {
            if surface
                .projectors
                .iter()
                .any(|projector| projector.name == topology.name())
            {
                return Err(ClientManifestError(format!(
                    "typed projected command `{}` topology `{}` does not own model `{}` on this client surface",
                    command.command_name,
                    topology.name(),
                    target.model
                )));
            }
            // A role surface may omit the whole projector when the same
            // topology also owns a denied model. The digest remains safe and
            // exact without revealing that hidden ownership inventory.
        }
        [owner] if owner.name == topology.name() => {
            if !target.topology_matches(
                &owner.name,
                &owner.facts,
                &owner.models,
                &owner.partition,
                owner.change_epoch.as_deref(),
            ) || !target.protocol_topology_matches(topology)
            {
                return Err(ClientManifestError(format!(
                    "typed projected command `{}` direct target differs from visible owner `{}`",
                    command.command_name, owner.name
                )));
            }
        }
        [owner] => {
            return Err(ClientManifestError(format!(
                "typed projected command `{}` names topology `{}` but visible owner `{}` owns model `{}`",
                command.command_name,
                topology.name(),
                owner.name,
                target.model
            )));
        }
        owners => {
            return Err(ClientManifestError(format!(
                "typed projected command `{}` model `{}` has ambiguous visible ownership: {}",
                command.command_name,
                target.model,
                owners
                    .iter()
                    .map(|owner| owner.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
    }

    let change_epoch = target.change_epoch.clone().ok_or_else(|| {
        ClientManifestError(format!(
            "typed projected command `{}` direct target has no registered change-log epoch",
            command.command_name
        ))
    })?;
    let partition = target
        .partition
        .as_ref()
        .map(serde_json::to_value)
        .transpose()?;
    let digest = topology
        .digest()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(Some(CommandDirectProjectionExtension {
        topology: ClientProjectionTopologyIdentity {
            version: topology.version(),
            name: topology.name().to_string(),
            digest: format!("sha256:{digest}"),
        },
        model: target.model.clone(),
        partition,
        change_epoch,
    }))
}

pub(crate) fn client_manifest_from_surface(
    service_id: &str,
    identity: ClientSurfaceIdentity,
    surface: &Surface,
) -> Result<DistributedClientManifest, ClientManifestError> {
    client_manifest_from_surface_with_execution(
        service_id,
        identity,
        surface,
        ClientExecutionLimits::default(),
    )
}

fn client_manifest_from_surface_with_execution(
    service_id: &str,
    identity: ClientSurfaceIdentity,
    surface: &Surface,
    execution: ClientExecutionLimits,
) -> Result<DistributedClientManifest, ClientManifestError> {
    if service_id.trim().is_empty() {
        return Err(ClientManifestError("service_id must not be empty".into()));
    }
    let identity = identity.canonicalized()?;
    match (&surface.selection, &identity) {
        (SurfaceSelection::Catalog, _) => {
            return Err(ClientManifestError(
                "client manifests require an explicitly role- or application-selected Surface"
                    .into(),
            ));
        }
        (SurfaceSelection::Role { name: selected }, ClientSurfaceIdentity::Role { name })
            if selected == name => {}
        (
            SurfaceSelection::Application {
                name: selected_name,
                roles: selected_roles,
            },
            ClientSurfaceIdentity::Application { name, roles },
        ) if selected_name == name && selected_roles == roles => {}
        _ => {
            return Err(ClientManifestError(
                "client Surface identity does not match its authorization selection provenance"
                    .into(),
            ));
        }
    }
    validate_surface_structure(surface)?;

    let live_models: BTreeSet<&str> = surface
        .subscription_fields
        .iter()
        .filter(|root| root.kind == RootKind::List)
        .map(|root| root.model_name.as_str())
        .collect();
    let mut models = Vec::new();
    for model in surface.models.values() {
        let mut fields: Vec<ClientField> = model
            .columns
            .iter()
            .map(|field| {
                let codec = scalar_codec(&field.scalar).ok_or_else(|| {
                    ClientManifestError(format!(
                        "model `{}` field `{}` uses unsupported scalar `{}`",
                        model.model_name, field.name, field.scalar
                    ))
                })?;
                Ok(ClientField {
                    name: field.name.clone(),
                    scalar: field.scalar.clone(),
                    codec: codec.into(),
                    nullable: field.nullable,
                })
            })
            .collect::<Result<_, ClientManifestError>>()?;
        fields.sort_by(|a, b| a.name.cmp(&b.name));
        let field_by_name: BTreeMap<&str, &ClientField> = fields
            .iter()
            .map(|field| (field.name.as_str(), field))
            .collect();
        let normalization = if model_has_client_normalized_identity(model) {
            ModelNormalization::Normalized {
                fields: model
                    .primary_key
                    .iter()
                    .map(|key| ClientKeyField {
                        name: key.clone(),
                        codec: field_by_name[key.as_str()].codec.clone(),
                    })
                    .collect(),
                encoding: KEY_ENCODING.into(),
            }
        } else {
            ModelNormalization::Embedded
        };

        let mut relationships = Vec::new();
        for relationship in &model.relationships {
            let Some(target) = surface.models.get(&relationship.target_model) else {
                return Err(ClientManifestError(format!(
                    "surface relationship `{}` targets absent model `{}`",
                    relationship.name, relationship.target_model
                )));
            };
            let target_field_by_name: BTreeMap<&str, &super::surface::ColumnField> = target
                .columns
                .iter()
                .map(|field| (field.name.as_str(), field))
                .collect();
            let source_field_by_name: BTreeMap<&str, &super::surface::ColumnField> = model
                .columns
                .iter()
                .map(|field| (field.name.as_str(), field))
                .collect();
            let stable_key_mapping = |local: &[String], remote: &[String]| {
                !local.is_empty()
                    && local.len() == remote.len()
                    && local.iter().all(|key| {
                        source_field_by_name
                            .get(key.as_str())
                            .is_some_and(|field| field.scalar != "BigInt")
                    })
                    && remote.iter().all(|key| {
                        target_field_by_name
                            .get(key.as_str())
                            .is_some_and(|field| field.scalar != "BigInt")
                    })
            };
            let key_mapping = match &relationship.keys {
                SurfaceRelationshipKeys::Direct { local, remote }
                    if stable_key_mapping(local, remote) =>
                {
                    RelationshipKeyMapping::Direct {
                        local: local.clone(),
                        remote: remote.clone(),
                    }
                }
                SurfaceRelationshipKeys::Through {
                    local,
                    remote,
                    table,
                    source_foreign_key,
                    target_foreign_key,
                } if stable_key_mapping(local, remote) => RelationshipKeyMapping::Through {
                    local: local.clone(),
                    remote: remote.clone(),
                    table: table.clone(),
                    source_foreign_key: source_foreign_key.clone(),
                    target_foreign_key: target_foreign_key.clone(),
                },
                SurfaceRelationshipKeys::ThroughOpaque {
                    local,
                    remote,
                    dependency,
                } if stable_key_mapping(local, remote) => RelationshipKeyMapping::ThroughOpaque {
                    local: local.clone(),
                    remote: remote.clone(),
                    dependency: dependency.clone(),
                },
                SurfaceRelationshipKeys::Embedded => RelationshipKeyMapping::Embedded,
                _ => RelationshipKeyMapping::Embedded,
            };
            let maintenance = match &key_mapping {
                RelationshipKeyMapping::Direct { .. } | RelationshipKeyMapping::Through { .. } => {
                    ClientRelationshipMaintenance::Local
                }
                RelationshipKeyMapping::ThroughOpaque { .. } | RelationshipKeyMapping::Embedded => {
                    ClientRelationshipMaintenance::Revalidate
                }
            };
            relationships.push(ClientRelationship {
                name: relationship.name.clone(),
                target_model: relationship.target_model.clone(),
                target_typename: relationship.target_object.clone(),
                kind: match relationship.kind {
                    RelationshipKind::HasMany => ClientRelationshipKind::HasMany,
                    RelationshipKind::BelongsTo => ClientRelationshipKind::BelongsTo,
                    RelationshipKind::ManyToMany => ClientRelationshipKind::ManyToMany,
                },
                list: relationship.list,
                nullable: relationship.nullable,
                arguments: relationship
                    .arguments
                    .iter()
                    .map(argument_manifest)
                    .collect(),
                key_mapping,
                maintenance,
                dependencies: sorted_unique(relationship.dependencies.clone()),
                filter: relationship.list.then(|| filter_semantics(surface, target)),
                order: relationship.list.then(|| order_semantics(target)),
                pagination: relationship
                    .list
                    .then(|| pagination_semantics(surface, target)),
                aggregate: relationship.aggregate.as_ref().map(|aggregate| {
                    ClientRelationshipAggregate {
                        name: aggregate.name.clone(),
                        arguments: aggregate.arguments.iter().map(argument_manifest).collect(),
                        semantics: aggregate_semantics(
                            target,
                            aggregate.type_name.clone(),
                            pagination_semantics(surface, target),
                        ),
                        dependencies: sorted_unique(aggregate.dependencies.clone()),
                    }
                }),
                live: relationship.list && live_models.contains(relationship.target_model.as_str()),
            });
        }
        relationships.sort_by(|a, b| a.name.cmp(&b.name));
        let filter_input = filter_input(surface, model)?;
        let causal_owner = model_has_visible_causal_owner(surface, &model.model_name);
        models.push(ClientModel {
            id: model.model_name.clone(),
            typename: model.object_name.clone(),
            source_table: model.table_name.clone(),
            dependencies: vec![model.table_name.clone()],
            normalization,
            fields,
            relationships,
            filter_input,
            row_policy: row_policy_manifest(&model.row_policy),
            // `_sourced_version` is only source metadata. These flags derive
            // from the role-visible Task 15 causal owner, never that column.
            record_revisions: causal_owner,
            tombstones: causal_owner,
        });
    }
    models.sort_by(|a, b| a.id.cmp(&b.id));

    let mut roots = Vec::new();
    for (operation, fields) in [
        (ClientRootOperation::Query, &surface.query_fields),
        (
            ClientRootOperation::Subscription,
            &surface.subscription_fields,
        ),
    ] {
        for root in fields {
            let model = surface.models.get(&root.model_name).ok_or_else(|| {
                ClientManifestError(format!(
                    "root `{}` references absent model `{}`",
                    root.name, root.model_name
                ))
            })?;
            let has_filter = root
                .arguments
                .iter()
                .any(|argument| argument.kind == SurfaceArgumentKind::Filter);
            let filter = has_filter.then(|| filter_semantics(surface, model));
            let has_order = root
                .arguments
                .iter()
                .any(|argument| argument.kind == SurfaceArgumentKind::Order);
            let order = has_order.then(|| order_semantics(model));
            let pagination =
                root.default_limit
                    .zip(root.max_limit)
                    .map(|(default_limit, max_limit)| ClientPaginationSemantics {
                        kind: "offset".into(),
                        default_limit,
                        max_limit,
                        coverage: "window".into(),
                    });
            let aggregate = matches!(root.kind, RootKind::Aggregate).then(|| {
                aggregate_semantics(
                    model,
                    aggregate_type_name(&model.schema),
                    pagination_semantics(surface, model),
                )
            });
            roots.push(ClientRoot {
                id: format!(
                    "{}:{}",
                    match operation {
                        ClientRootOperation::Query => "query",
                        ClientRootOperation::Subscription => "subscription",
                    },
                    root.name
                ),
                operation,
                name: root.name.clone(),
                kind: match root.kind {
                    RootKind::List => ClientRootKind::List,
                    RootKind::ByPk => ClientRootKind::ByPk,
                    RootKind::Aggregate => ClientRootKind::Aggregate,
                },
                model: root.model_name.clone(),
                arguments: root.arguments.iter().map(argument_manifest).collect(),
                filter,
                order,
                pagination,
                aggregate,
                dependencies: sorted_unique(root.dependencies.clone()),
                live: operation == ClientRootOperation::Subscription
                    || surface.subscription_fields.iter().any(|candidate| {
                        candidate.name == root.name && candidate.kind == root.kind
                    }),
            });
        }
    }
    roots.sort_by(|a, b| a.id.cmp(&b.id));

    let mut commands = Vec::new();
    for command in surface
        .commands
        .iter()
        .filter(|command| command.consistency.is_some())
    {
        // Legacy GraphqlCommands may still be useful for server-only schema
        // migration, but they are not bound to the executable typed causal
        // inventory and therefore never enter a generated client contract.
        let input = command_shape(&command.input)?;
        let output = command_shape(&command.output)?;
        let grants = sorted_unique(command.roles.clone());
        let operation = command_operation(&command.field_name, &input, &output);
        let operation_hash = hash_bytes(operation.as_bytes());
        let consistency = command.consistency.map(|kind| CommandConsistencyExtension {
            version: 1,
            kind: match kind {
                CommandConsistency::Accepted => "accepted",
                CommandConsistency::Fact => "fact",
                CommandConsistency::Projected => "projected",
            }
            .into(),
        });
        let direct_projection = command_direct_projection_extension(command, surface)?;
        let input_defaults = (!command.input_defaults.is_empty())
            .then(|| {
                command
                    .input_defaults
                    .iter()
                    .map(serde_json::to_value)
                    .collect::<Result<Vec<_>, _>>()
                    .map(|defaults| CommandInputDefaultsExtension {
                        version: 1,
                        defaults,
                    })
            })
            .transpose()?;
        let effects = command
            .effects
            .as_ref()
            .map(|effects| {
                let operations = effects
                    .operations
                    .iter()
                    .map(serde_json::to_value)
                    .collect::<Result<Vec<_>, _>>()?;
                let fallback = match effects.fallback {
                    CommandEffectFallback::Revalidate => "revalidate",
                };
                Ok::<_, serde_json::Error>(CommandEffectsExtension {
                    version: 1,
                    operations,
                    fallback: fallback.into(),
                })
            })
            .transpose()?;
        let confirmations = if command.confirmation_unavailable {
            Some(CommandConfirmationsExtension {
                version: COMMAND_CONFIRMATIONS_VERSION,
                kind: "unavailable".into(),
                expected: Vec::new(),
                fallback: "revalidate".into(),
            })
        } else {
            (!command.confirmations.is_empty()
                || matches!(command.consistency, Some(CommandConsistency::Fact)))
            .then(|| {
                command
                    .confirmations
                    .iter()
                    .map(serde_json::to_value)
                    .collect::<Result<Vec<_>, _>>()
                    .map(|expected| CommandConfirmationsExtension {
                        version: COMMAND_CONFIRMATIONS_VERSION,
                        kind: "finite".into(),
                        expected,
                        fallback: "revalidate".into(),
                    })
            })
            .transpose()?
        };
        commands.push(ClientCommand {
            version: 1,
            name: command.command_name.clone(),
            mutation_field: command.field_name.clone(),
            grants,
            input,
            output,
            operation,
            operation_hash,
            extensions: ClientCommandExtensionSlots {
                version: COMMAND_EXTENSION_SLOTS_VERSION,
                consistency,
                direct_projection,
                input_defaults,
                effects,
                confirmations,
            },
        });
    }
    commands.sort_by(|a, b| a.name.cmp(&b.name));

    let command_status = (!commands.is_empty()).then(|| {
        let operation = command_status_operation();
        ClientProtocolOperation {
            name: "Distributed_CommandStatus".into(),
            operation_hash: hash_bytes(operation.as_bytes()),
            operation,
        }
    });
    let protocol_operations = ClientProtocolOperations {
        version: PROTOCOL_OPERATIONS_VERSION,
        command_status,
    };

    let confirming_projectors: BTreeSet<&str> = surface
        .commands
        .iter()
        .flat_map(|command| {
            command
                .confirmations
                .iter()
                .map(|confirmation| confirmation.projector.as_str())
        })
        .collect();
    let mut projectors: Vec<ClientProjector> = surface
        .projectors
        .iter()
        .map(|projector| ClientProjector {
            version: PROJECTOR_ENTRY_VERSION,
            name: projector.name.clone(),
            facts: sorted_unique(projector.facts.clone()),
            models: sorted_unique(projector.models.clone()),
            dependencies: sorted_unique(projector.dependencies.clone()),
            causal_confirmation: confirming_projectors.contains(projector.name.as_str()),
        })
        .collect();
    projectors.sort_by(|a, b| a.name.cmp(&b.name));

    let scalar_codecs = supported_scalar_codecs();
    let record_evidence = query_footprint_has_record_evidence(surface);
    let capabilities = ClientCapabilities {
        live_queries: !surface.subscription_fields.is_empty(),
        record_revisions: record_evidence,
        tombstones: record_evidence,
        causal_receipts: !commands.is_empty(),
        live_resume: query_footprint_supports_live_resume(surface),
        query_fallback: "revalidate".into(),
        // Task 16 requires a scope envelope for generated causal receipts.
        // Query-only scope generation, SSR, hydration, and persistence remain
        // capability-disabled until task 10 wires that lifecycle end to end.
        cache_scope: !commands.is_empty(),
        confirmed_persistence: false,
    };
    let protocol_fingerprint = protocol_fingerprint()?;

    #[derive(Serialize)]
    struct SchemaMaterial<'a> {
        manifest_version: u32,
        protocol_version: u32,
        service_id: &'a str,
        surface: &'a ClientSurfaceIdentity,
        execution: &'a ClientExecutionLimits,
        capabilities: &'a ClientCapabilities,
        scalar_codecs: &'a [ScalarCodec],
        models: &'a [ClientModel],
        roots: &'a [ClientRoot],
        commands: &'a [ClientCommand],
        protocol_operations: &'a ClientProtocolOperations,
        projectors: &'a [ClientProjector],
    }
    let schema_fingerprint = hash_json(&SchemaMaterial {
        manifest_version: DISTRIBUTED_CLIENT_MANIFEST_VERSION,
        protocol_version: DISTRIBUTED_CLIENT_PROTOCOL_VERSION,
        service_id,
        surface: &identity,
        execution: &execution,
        capabilities: &capabilities,
        scalar_codecs: &scalar_codecs,
        models: &models,
        roots: &roots,
        commands: &commands,
        protocol_operations: &protocol_operations,
        projectors: &projectors,
    })?;

    Ok(DistributedClientManifest {
        manifest_version: DISTRIBUTED_CLIENT_MANIFEST_VERSION,
        protocol_version: DISTRIBUTED_CLIENT_PROTOCOL_VERSION,
        service_id: service_id.into(),
        surface: identity,
        schema_fingerprint,
        protocol_fingerprint,
        execution,
        capabilities,
        scalar_codecs,
        models,
        roots,
        commands,
        protocol_operations,
        projectors,
    })
}

fn validate_surface_structure(surface: &Surface) -> Result<(), ClientManifestError> {
    fn unique_nonempty<'a>(
        values: impl IntoIterator<Item = &'a str>,
        label: &str,
    ) -> Result<(), ClientManifestError> {
        let mut seen = BTreeSet::new();
        for value in values {
            if value.trim().is_empty() {
                return Err(ClientManifestError(format!("{label} id must not be empty")));
            }
            if !seen.insert(value) {
                return Err(ClientManifestError(format!(
                    "duplicate {label} id `{value}`"
                )));
            }
        }
        Ok(())
    }

    unique_nonempty(
        surface
            .models
            .values()
            .map(|model| model.model_name.as_str()),
        "model",
    )?;
    unique_nonempty(
        surface
            .models
            .values()
            .map(|model| model.table_name.as_str()),
        "model source table",
    )?;
    unique_nonempty(
        surface
            .models
            .values()
            .map(|model| model.object_name.as_str()),
        "model typename",
    )?;
    for (key, model) in &surface.models {
        if key != &model.model_name {
            return Err(ClientManifestError(format!(
                "surface model map key `{key}` does not match model id `{}`",
                model.model_name
            )));
        }
        unique_nonempty(
            model.columns.iter().map(|field| field.name.as_str()),
            "field",
        )?;
        unique_nonempty(
            model.relationships.iter().map(|field| field.name.as_str()),
            "relationship",
        )?;
        if let SurfaceRowPolicy::Predicate(predicate) = &model.row_policy {
            predicate
                .validate_row_policy_literals()
                .map_err(ClientManifestError)?;
            if !predicate.is_client_portable() {
                return Err(ClientManifestError(format!(
                    "model `{}` exposes a row policy with a JavaScript-unsafe integer; select it through surface_for_role so it becomes server-only",
                    model.model_name
                )));
            }
        }
        for relationship in &model.relationships {
            if !surface.models.contains_key(&relationship.target_model) {
                return Err(ClientManifestError(format!(
                    "model `{}` relationship `{}` targets absent model `{}`",
                    model.model_name, relationship.name, relationship.target_model
                )));
            }
            unique_nonempty(
                relationship.dependencies.iter().map(String::as_str),
                &format!(
                    "model `{}` relationship `{}` dependency",
                    model.model_name, relationship.name
                ),
            )?;
        }
    }
    unique_nonempty(
        surface.query_fields.iter().map(|root| root.name.as_str()),
        "query root",
    )?;
    unique_nonempty(
        surface
            .subscription_fields
            .iter()
            .map(|root| root.name.as_str()),
        "subscription root",
    )?;
    unique_nonempty(
        surface
            .commands
            .iter()
            .map(|command| command.command_name.as_str()),
        "command",
    )?;
    unique_nonempty(
        surface
            .commands
            .iter()
            .map(|command| command.field_name.as_str()),
        "command mutation field",
    )?;
    for command in &surface.commands {
        unique_nonempty(
            command.roles.iter().map(String::as_str),
            &format!("command `{}` role", command.command_name),
        )?;
    }
    unique_nonempty(
        surface
            .projectors
            .iter()
            .map(|projector| projector.name.as_str()),
        "projector",
    )?;
    for projector in &surface.projectors {
        unique_nonempty(
            projector.facts.iter().map(String::as_str),
            &format!("projector `{}` fact", projector.name),
        )?;
        unique_nonempty(
            projector.models.iter().map(String::as_str),
            &format!("projector `{}` model", projector.name),
        )?;
        for model in &projector.models {
            if !surface.models.contains_key(model) {
                return Err(ClientManifestError(format!(
                    "projector `{}` targets absent model `{model}`",
                    projector.name
                )));
            }
        }
    }
    Ok(())
}

fn row_policy_manifest(policy: &SurfaceRowPolicy) -> ClientRowPolicy {
    match policy {
        SurfaceRowPolicy::Unrestricted => ClientRowPolicy::Unrestricted,
        SurfaceRowPolicy::Predicate(predicate) => ClientRowPolicy::Predicate {
            expression: predicate.clone(),
        },
        SurfaceRowPolicy::ServerOnly => ClientRowPolicy::ServerOnly,
    }
}

fn filter_semantics(
    surface: &Surface,
    model: &super::surface::SurfaceModel,
) -> ClientFilterSemantics {
    ClientFilterSemantics {
        fields: filter_fields(surface, model),
        relationships: filter_relationship_names(model),
        row_policy: row_policy_manifest(&model.row_policy),
    }
}

fn filter_input(
    surface: &Surface,
    model: &super::surface::SurfaceModel,
) -> Result<ClientFilterInput, ClientManifestError> {
    let mut relationships = model
        .relationships
        .iter()
        .map(|relationship| {
            let target = surface
                .models
                .get(&relationship.target_model)
                .ok_or_else(|| {
                    ClientManifestError(format!(
                        "model `{}` filter relationship `{}` targets absent model `{}`",
                        model.model_name, relationship.name, relationship.target_model
                    ))
                })?;
            Ok(ClientFilterInputRelationship {
                field: relationship.name.clone(),
                target_type: format!("{}_bool_exp", target.table_name),
            })
        })
        .collect::<Result<Vec<_>, ClientManifestError>>()?;
    relationships.sort_by(|left, right| left.field.cmp(&right.field));
    Ok(ClientFilterInput {
        type_name: format!("{}_bool_exp", model.table_name),
        fields: filter_fields(surface, model),
        relationships,
    })
}

fn filter_fields(
    surface: &Surface,
    model: &super::surface::SurfaceModel,
) -> Vec<ClientFilterField> {
    let mut fields: Vec<ClientFilterField> = model
        .columns
        .iter()
        .map(|field| ClientFilterField {
            name: field.name.clone(),
            operators: surface
                .comparison_ops_for_scalar(&field.scalar)
                .into_iter()
                .map(str::to_string)
                .collect(),
        })
        .collect();
    fields.sort_by(|a, b| a.name.cmp(&b.name));
    fields
}

fn filter_relationship_names(model: &super::surface::SurfaceModel) -> Vec<String> {
    let mut relationships: Vec<String> = model
        .relationships
        .iter()
        .map(|relationship| relationship.name.clone())
        .collect();
    relationships.sort();
    relationships
}

fn order_semantics(model: &super::surface::SurfaceModel) -> ClientOrderSemantics {
    let mut fields: Vec<String> = model
        .columns
        .iter()
        .map(|field| field.name.clone())
        .collect();
    fields.sort();
    ClientOrderSemantics {
        fields,
        values: [
            "asc",
            "asc_nulls_first",
            "asc_nulls_last",
            "desc",
            "desc_nulls_first",
            "desc_nulls_last",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
    }
}

fn pagination_semantics(
    surface: &Surface,
    model: &super::surface::SurfaceModel,
) -> ClientPaginationSemantics {
    let max_limit = model
        .role_limit
        .unwrap_or(surface.max_limit)
        .min(surface.max_limit);
    ClientPaginationSemantics {
        kind: "offset".into(),
        default_limit: surface.default_limit.min(max_limit),
        max_limit,
        coverage: "window".into(),
    }
}

fn aggregate_semantics(
    model: &super::surface::SurfaceModel,
    wrapper_typename: String,
    nodes_pagination: ClientPaginationSemantics,
) -> ClientAggregateSemantics {
    ClientAggregateSemantics {
        wrapper_typename,
        fields_typename: aggregate_fields_type_name(&model.schema),
        nodes_pagination,
        count: true,
        nodes: true,
        sum: Vec::new(),
        avg: Vec::new(),
        min: Vec::new(),
        max: Vec::new(),
    }
}

fn argument_manifest(argument: &SurfaceArgument) -> ClientArgument {
    ClientArgument {
        name: argument.name.clone(),
        kind: match argument.kind {
            SurfaceArgumentKind::Filter => ClientArgumentKind::Filter,
            SurfaceArgumentKind::Order => ClientArgumentKind::Order,
            SurfaceArgumentKind::Limit => ClientArgumentKind::Limit,
            SurfaceArgumentKind::Offset => ClientArgumentKind::Offset,
            SurfaceArgumentKind::PrimaryKey => ClientArgumentKind::PrimaryKey,
        },
        type_name: argument.type_name.clone(),
        nullable: argument.nullable,
        list: argument.list,
        codec: scalar_codec(&argument.type_name).map(str::to_string),
    }
}

fn command_shape(shape: &SurfaceCommandShape) -> Result<ClientCommandShape, ClientManifestError> {
    match shape {
        SurfaceCommandShape::None => Ok(ClientCommandShape::None),
        SurfaceCommandShape::Json => Ok(ClientCommandShape::Json {
            codec: scalar_codec("JSON").expect("JSON codec").into(),
        }),
        SurfaceCommandShape::Typed(definition) => Ok(ClientCommandShape::Object {
            definition: client_type(definition)?,
        }),
    }
}

fn client_type(definition: &SurfaceTypeDef) -> Result<ClientTypeDef, ClientManifestError> {
    let mut fields = Vec::new();
    for field in &definition.fields {
        if !field.list && field.item_nullable {
            return Err(ClientManifestError(format!(
                "command type `{}` field `{}` marks non-list items nullable",
                definition.name, field.name
            )));
        }
        let nested = field
            .nested
            .as_deref()
            .map(client_type)
            .transpose()?
            .map(Box::new);
        let codec = scalar_codec(&field.type_name).map(str::to_string);
        if codec.is_none() && nested.is_none() {
            return Err(ClientManifestError(format!(
                "command type `{}` field `{}` uses unknown scalar/object `{}`",
                definition.name, field.name, field.type_name
            )));
        }
        fields.push(ClientTypeField {
            name: field.name.clone(),
            type_name: field.type_name.clone(),
            nullable: field.nullable,
            list: field.list,
            item_nullable: field.item_nullable,
            codec,
            nested,
        });
    }
    fields.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(ClientTypeDef {
        name: definition.name.clone(),
        fields,
    })
}

fn command_operation(
    mutation_field: &str,
    input: &ClientCommandShape,
    output: &ClientCommandShape,
) -> String {
    let operation_name = format!("Client_{mutation_field}");
    let (variables, arguments) = match input {
        ClientCommandShape::None => (
            "($commandId: ID!)".to_string(),
            "(commandId: $commandId)".to_string(),
        ),
        ClientCommandShape::Json { .. } => (
            "($commandId: ID!, $input: JSON!)".to_string(),
            "(commandId: $commandId, input: $input)".to_string(),
        ),
        ClientCommandShape::Object { definition } => (
            format!("($commandId: ID!, $input: {}!)", definition.name),
            "(commandId: $commandId, input: $input)".to_string(),
        ),
    };
    let selection = match output {
        ClientCommandShape::Object { definition } => {
            format!(" {{ {} }}", command_selection(definition))
        }
        ClientCommandShape::None | ClientCommandShape::Json { .. } => String::new(),
    };
    format!("mutation {operation_name}{variables} {{ {mutation_field}{arguments}{selection} }}")
}

fn command_status_operation() -> String {
    "query Distributed_CommandStatus($commandId: ID!) { commandStatus(commandId: $commandId) { state } }"
        .into()
}

fn command_selection(definition: &ClientTypeDef) -> String {
    definition
        .fields
        .iter()
        .map(|field| match &field.nested {
            Some(nested) => format!("{} {{ {} }}", field.name, command_selection(nested)),
            None => field.name.clone(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn supported_scalar_codecs() -> Vec<ScalarCodec> {
    [
        // SQL JSON projection currently emits integer JSON numbers. Browsers
        // must treat values outside the JS safe-integer range as lossy.
        ("BigInt", "json_number_precision_limited"),
        ("Boolean", "boolean"),
        ("Bytea", "base64"),
        ("Float", "float64"),
        ("ID", "string"),
        ("Int", "int32"),
        ("JSON", "json"),
        ("String", "string"),
        // Both dialects emit a string, but the framework does not normalize or
        // validate RFC3339 at this layer yet.
        ("Timestamptz", "string_unvalidated_timestamp"),
    ]
    .into_iter()
    .map(|(scalar, codec)| ScalarCodec {
        scalar: scalar.into(),
        codec: codec.into(),
    })
    .collect()
}

fn scalar_codec(scalar: &str) -> Option<&'static str> {
    match scalar {
        "BigInt" => Some("json_number_precision_limited"),
        "Boolean" => Some("boolean"),
        "Bytea" => Some("base64"),
        "Float" => Some("float64"),
        "ID" | "String" => Some("string"),
        "Int" => Some("int32"),
        "JSON" => Some("json"),
        "Timestamptz" => Some("string_unvalidated_timestamp"),
        _ => None,
    }
}

fn protocol_fingerprint() -> Result<String, ClientManifestError> {
    #[derive(Serialize)]
    struct ProtocolMaterial {
        manifest_version: u32,
        protocol_version: u32,
        key_encoding: &'static str,
        command_extension_slots_version: u32,
        projector_entry_version: u32,
        protocol_operations_version: u32,
        query_capabilities_version: u32,
        scalar_codecs: Vec<ScalarCodec>,
    }
    hash_json(&ProtocolMaterial {
        manifest_version: DISTRIBUTED_CLIENT_PROTOCOL_MANIFEST_EPOCH,
        protocol_version: DISTRIBUTED_CLIENT_PROTOCOL_VERSION,
        key_encoding: KEY_ENCODING,
        command_extension_slots_version: COMMAND_EXTENSION_SLOTS_VERSION,
        projector_entry_version: PROJECTOR_ENTRY_VERSION,
        protocol_operations_version: PROTOCOL_OPERATIONS_VERSION,
        query_capabilities_version: QUERY_CAPABILITIES_VERSION,
        scalar_codecs: supported_scalar_codecs(),
    })
}

fn hash_json(value: &impl Serialize) -> Result<String, ClientManifestError> {
    let bytes = serde_json::to_vec(value)?;
    Ok(hash_bytes(&bytes))
}

fn hash_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("sha256:{digest:x}")
}

fn sorted_unique(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values.dedup();
    values
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphql::{
        build_surface, claim, col, exposed_command, rel, surface_for_application, surface_for_role,
        typed_command, Accepted, GraphqlCommands, GraphqlInputType, GraphqlOutputType,
        GraphqlTypeDef, GraphqlTypeField, PreparedCommand, RoleGrant, SurfaceCommand,
        SurfaceOptions, SurfaceProjector, SurfaceTypeField,
    };
    use crate::microsvc::{CausalCommandContext, HandlerError, Routes, Service};
    use crate::table::{
        ColumnType, PrimaryKey, RelationshipDef, RelationshipKind, TableColumn, TableKind,
        TableSchema,
    };
    use std::any::TypeId;

    #[test]
    fn generated_causal_command_operation_requires_framework_command_id() {
        let operation = command_operation(
            "todo_create",
            &ClientCommandShape::Object {
                definition: ClientTypeDef {
                    name: "CreateTodoInput".into(),
                    fields: Vec::new(),
                },
            },
            &ClientCommandShape::Object {
                definition: ClientTypeDef {
                    name: "CreateTodoOutput".into(),
                    fields: vec![ClientTypeField {
                        name: "id".into(),
                        type_name: "String".into(),
                        nullable: false,
                        list: false,
                        item_nullable: false,
                        codec: Some("string".into()),
                        nested: None,
                    }],
                },
            },
        );
        assert_eq!(
            operation,
            "mutation Client_todo_create($commandId: ID!, $input: CreateTodoInput!) { todo_create(commandId: $commandId, input: $input) { id } }"
        );
    }

    fn column(name: &str, ty: ColumnType) -> TableColumn {
        TableColumn::new(name, name, ty)
    }

    fn users() -> TableSchema {
        TableSchema {
            model_name: "UserView".into(),
            table_name: "users".into(),
            columns: vec![
                TableColumn {
                    primary_key: true,
                    ..column("user_id", ColumnType::Text)
                },
                column("display_name", ColumnType::Text),
                column("secret", ColumnType::Text),
            ],
            primary_key: PrimaryKey::new(["user_id"]),
            version_column: Some("_sourced_version".into()),
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships: Vec::new(),
            kind: TableKind::ReadModel,
        }
    }

    fn todos() -> TableSchema {
        TableSchema {
            model_name: "TodoView".into(),
            table_name: "todos".into(),
            columns: vec![
                TableColumn {
                    primary_key: true,
                    ..column("todo_id", ColumnType::Text)
                },
                column("owner_id", ColumnType::Text),
                column("title", ColumnType::Text),
                column("completed", ColumnType::Boolean),
            ],
            primary_key: PrimaryKey::new(["todo_id"]),
            version_column: Some("_sourced_version".into()),
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships: vec![RelationshipDef {
                field_name: "owner".into(),
                kind: RelationshipKind::BelongsTo,
                target_model: "UserView".into(),
                foreign_key: Some("owner_id".into()),
                through: None,
                target_foreign_key: None,
            }],
            kind: TableKind::ReadModel,
        }
    }

    fn memberships() -> TableSchema {
        TableSchema {
            model_name: "MembershipView".into(),
            table_name: "memberships".into(),
            columns: vec![
                TableColumn {
                    primary_key: true,
                    ..column("tenant_id", ColumnType::Text)
                },
                TableColumn {
                    primary_key: true,
                    ..column("user_id", ColumnType::Text)
                },
                column("role", ColumnType::Text),
            ],
            primary_key: PrimaryKey::new(["tenant_id", "user_id"]),
            version_column: None,
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships: Vec::new(),
            kind: TableKind::ReadModel,
        }
    }

    fn teams() -> TableSchema {
        TableSchema {
            model_name: "TeamView".into(),
            table_name: "teams".into(),
            columns: vec![
                TableColumn {
                    primary_key: true,
                    ..column("team_id", ColumnType::Text)
                },
                column("name", ColumnType::Text),
            ],
            primary_key: PrimaryKey::new(["team_id"]),
            version_column: None,
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships: vec![RelationshipDef {
                field_name: "members".into(),
                kind: RelationshipKind::ManyToMany,
                target_model: "UserView".into(),
                foreign_key: Some("team_id".into()),
                through: Some("private_team_members".into()),
                target_foreign_key: Some("user_id".into()),
            }],
            kind: TableKind::ReadModel,
        }
    }

    fn team_members() -> TableSchema {
        TableSchema {
            model_name: "PrivateTeamMember".into(),
            table_name: "private_team_members".into(),
            columns: vec![
                TableColumn {
                    primary_key: true,
                    ..column("team_id", ColumnType::Text)
                },
                TableColumn {
                    primary_key: true,
                    ..column("user_id", ColumnType::Text)
                },
            ],
            primary_key: PrimaryKey::new(["team_id", "user_id"]),
            version_column: None,
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships: Vec::new(),
            kind: TableKind::Operational,
        }
    }

    #[derive(Deserialize)]
    struct CompleteInput;
    impl GraphqlInputType for CompleteInput {
        fn graphql_type() -> GraphqlTypeDef {
            GraphqlTypeDef::new(
                "CompleteTodoInput",
                vec![GraphqlTypeField {
                    name: "todo_id".into(),
                    type_name: "String".into(),
                    nullable: false,
                    list: false,
                    item_nullable: false,
                    nested: None,
                }],
            )
            .with_type_id(TypeId::of::<Self>())
        }
    }

    #[derive(Serialize)]
    struct CompletePayload;
    impl GraphqlOutputType for CompletePayload {
        fn graphql_type() -> GraphqlTypeDef {
            GraphqlTypeDef::new(
                "CompleteTodoPayload",
                vec![GraphqlTypeField {
                    name: "todo_id".into(),
                    type_name: "String".into(),
                    nullable: false,
                    list: false,
                    item_nullable: false,
                    nested: None,
                }],
            )
            .with_type_id(TypeId::of::<Self>())
        }
    }

    #[derive(Default)]
    struct ManifestAggregate {
        entity: crate::Entity,
    }

    impl crate::Aggregate for ManifestAggregate {
        type ReplayError = std::convert::Infallible;

        fn entity(&self) -> &crate::Entity {
            &self.entity
        }

        fn entity_mut(&mut self) -> &mut crate::Entity {
            &mut self.entity
        }

        fn replay_event(&mut self, _event: &crate::EventRecord) -> Result<(), Self::ReplayError> {
            Ok(())
        }
    }

    async fn complete_handler(
        _context: &CausalCommandContext<'_, ManifestAggregate>,
        _input: CompleteInput,
    ) -> Result<PreparedCommand<Accepted<CompletePayload>>, HandlerError> {
        Ok(
            PreparedCommand::<Accepted<CompletePayload>>::prepare(CompletePayload)
                .expect("serializable command payload"),
        )
    }

    fn full_surface() -> Surface {
        let service = Service::new().named("todos-service").routes(
            Routes::new()
                .with_repo(crate::AggregateRepository::<_, ManifestAggregate>::new(
                    crate::InMemoryRepository::new(),
                ))
                .typed_command(
                    typed_command::<CompleteInput, Accepted<CompletePayload>>("todo.complete")
                        .field_name("todos_complete")
                        .roles(["admin", "user"]),
                )
                .handle(complete_handler)
                .typed_command(
                    typed_command::<CompleteInput, Accepted<CompletePayload>>("todo.force_archive")
                        .field_name("todos_force_archive")
                        .roles(["admin"]),
                )
                .handle(complete_handler),
        );
        build_surface(
            &[todos(), users(), memberships()],
            &SurfaceOptions::sqlite(),
        )
        .expect("surface")
        .with_service(&service)
        .expect("typed service")
        .with_projectors([
            SurfaceProjector::new("project_todos")
                .facts(["todo.completed"])
                .models(["TodoView"]),
            SurfaceProjector::new("project_users")
                .facts(["user.changed"])
                .models(["UserView"]),
        ])
        .expect("projectors")
    }

    fn projected_surface() -> Surface {
        use super::super::command_contract::{
            CommandEffects, CommandProjectedModel, EffectExpression,
        };

        let todo_schema: &'static TableSchema = Box::leak(Box::new(todos()));
        let mut surface = build_surface(&[todo_schema.clone(), users()], &SurfaceOptions::sqlite())
            .expect("projected surface");
        let todo_model = surface.models["TodoView"].clone();
        let mut output_fields = todo_model
            .columns
            .iter()
            .map(|column| SurfaceTypeField {
                name: column.name.clone(),
                type_name: column.scalar.clone(),
                nullable: column.nullable,
                list: false,
                item_nullable: false,
                nested: None,
            })
            .collect::<Vec<_>>();
        output_fields.sort_by(|left, right| left.name.cmp(&right.name));
        surface.commands = vec![SurfaceCommand {
            command_name: "todo.project".into(),
            field_name: "todo_project".into(),
            roles: vec!["user".into()],
            input: SurfaceCommandShape::Typed(SurfaceTypeDef {
                name: "ProjectTodoInput".into(),
                fields: vec![SurfaceTypeField {
                    name: "todo_id".into(),
                    type_name: "String".into(),
                    nullable: false,
                    list: false,
                    item_nullable: false,
                    nested: None,
                }],
            }),
            output: SurfaceCommandShape::Typed(SurfaceTypeDef {
                name: todo_model.object_name,
                fields: output_fields,
            }),
            consistency: Some(CommandConsistency::Projected),
            input_defaults: Vec::new(),
            effects: Some(CommandEffects::revalidate()),
            confirmations: Vec::new(),
            projected_model: Some(CommandProjectedModel {
                output_type_id: TypeId::of::<()>(),
                model: "TodoView".into(),
                table: "todos".into(),
                schema: todo_schema,
                partition: Some(EffectExpression::Input {
                    path: vec!["todo_id".into()],
                }),
            }),
            direct_projection: None,
            confirmation_unavailable: false,
        }];
        surface.commands_attached = true;
        surface
            .with_projectors([SurfaceProjector::new("project_todo_domain")
                .facts(["todo.changed", "user.changed"])
                .models(["TodoView", "UserView"])
                .partition_by(["todo_id"])
                .change_epoch("todo-domain-v1")])
            .expect("projected topology")
    }

    #[test]
    fn projected_command_exports_opaque_role_safe_direct_target() {
        let full = projected_surface();
        let selected = surface_for_role(
            &full,
            "user",
            &BTreeMap::from([("TodoView".into(), RoleGrant::all_columns())]),
        )
        .expect("role selection");
        assert!(
            selected.projectors.is_empty(),
            "the multi-model owner must be omitted when one owned model is denied"
        );

        let manifest = client_manifest_from_surface(
            "todos-service",
            ClientSurfaceIdentity::role("user"),
            &selected,
        )
        .expect("projected client manifest");
        let direct = manifest.commands[0]
            .extensions
            .direct_projection
            .as_ref()
            .expect("projected direct target");
        assert_eq!(direct.topology.version, 1);
        assert_eq!(direct.topology.name, "project_todo_domain");
        assert_eq!(direct.topology.digest.len(), 71);
        assert!(direct.topology.digest.starts_with("sha256:"));
        assert!(direct.topology.digest[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
        assert_eq!(direct.model, "TodoView");
        assert_eq!(
            direct.partition,
            Some(serde_json::json!({"kind": "input", "path": ["todo_id"]}))
        );
        assert_eq!(direct.change_epoch, "todo-domain-v1");

        let direct_wire = serde_json::to_value(direct).expect("direct target wire");
        assert_eq!(
            direct_wire
                .as_object()
                .expect("direct target object")
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["change_epoch", "model", "partition", "topology"])
        );
        let wire = serde_json::to_string(&manifest).expect("client manifest wire");
        assert!(!wire.contains("UserView"));
        assert!(!wire.contains("users"));
        assert!(!wire.contains("user.changed"));
    }

    #[test]
    fn projected_command_export_rejects_absent_or_wrong_direct_target() {
        let full = projected_surface();
        let mut selected = surface_for_role(
            &full,
            "user",
            &BTreeMap::from([
                ("TodoView".into(), RoleGrant::all_columns()),
                ("UserView".into(), RoleGrant::all_columns()),
            ]),
        )
        .expect("role selection");

        selected.commands[0].direct_projection = None;
        let error = client_manifest_from_surface(
            "todos-service",
            ClientSurfaceIdentity::role("user"),
            &selected,
        )
        .expect_err("projected command cannot omit direct target");
        assert!(error
            .0
            .contains("missing its bound direct projection target"));

        let mut selected = surface_for_role(
            &full,
            "user",
            &BTreeMap::from([
                ("TodoView".into(), RoleGrant::all_columns()),
                ("UserView".into(), RoleGrant::all_columns()),
            ]),
        )
        .expect("role selection");
        selected.commands[0]
            .direct_projection
            .as_mut()
            .expect("bound target")
            .model = "UserView".into();
        let error = client_manifest_from_surface(
            "todos-service",
            ClientSurfaceIdentity::role("user"),
            &selected,
        )
        .expect_err("direct target cannot name a different retained model");
        assert!(error.0.contains("differs from retained model"));
    }

    #[test]
    fn client_manifest_exports_only_bound_typed_causal_commands() {
        let mut commands = GraphqlCommands::from_typed_contracts(&[typed_command::<
            CompleteInput,
            Accepted<CompletePayload>,
        >("todo.complete")
        .into_contract()])
        .expect("typed command");
        commands = commands.command(
            "legacy.internal",
            exposed_command()
                .input::<CompleteInput>()
                .output::<CompletePayload>(),
        );
        let catalog = build_surface(&[], &SurfaceOptions::sqlite())
            .unwrap()
            .with_commands(&commands)
            .unwrap();
        let selected = surface_for_role(&catalog, "anonymous", &BTreeMap::new()).unwrap();
        let manifest = client_manifest_from_surface(
            "todos",
            ClientSurfaceIdentity::role("anonymous"),
            &selected,
        )
        .unwrap();

        assert_eq!(
            manifest
                .commands
                .iter()
                .map(|command| command.name.as_str())
                .collect::<Vec<_>>(),
            vec!["todo.complete"]
        );
    }

    fn grants() -> BTreeMap<String, BTreeMap<String, RoleGrant>> {
        BTreeMap::from([
            (
                "admin".into(),
                BTreeMap::from([
                    (
                        "TodoView".into(),
                        RoleGrant::all_columns().with_aggregations(),
                    ),
                    (
                        "UserView".into(),
                        RoleGrant::all_columns().with_aggregations(),
                    ),
                    ("MembershipView".into(), RoleGrant::all_columns()),
                ]),
            ),
            (
                "user".into(),
                BTreeMap::from([
                    (
                        "TodoView".into(),
                        RoleGrant::all_columns()
                            .rows(col("owner_id").eq(claim("x-user-id")))
                            .limit(25),
                    ),
                    ("UserView".into(), RoleGrant::columns(["display_name"])),
                    (
                        "MembershipView".into(),
                        RoleGrant::columns(["tenant_id", "role"]),
                    ),
                ]),
            ),
        ])
    }

    fn manifest_for_all_models(
        service_id: &str,
        role: &str,
        full: &Surface,
    ) -> DistributedClientManifest {
        let grants = full
            .models
            .keys()
            .map(|model| (model.clone(), RoleGrant::all_columns().with_aggregations()))
            .collect();
        let selected = surface_for_role(full, role, &grants).expect("role surface");
        client_manifest_from_surface(service_id, ClientSurfaceIdentity::role(role), &selected)
            .expect("client manifest")
    }

    #[test]
    fn role_manifest_is_deterministic_and_hides_denied_identity_and_commands() {
        let full = full_surface();
        let selected = surface_for_role(&full, "user", &grants()["user"]).unwrap();
        let export = DistributedClientSurfaceExport::from_selected("todos-service", selected)
            .expect("role-selected Surface");
        let first = export.manifest().unwrap();
        let second = export.manifest().unwrap();
        assert_eq!(first, second);
        assert_eq!(first.manifest_version, 6);
        assert_eq!(first.schema_fingerprint, second.schema_fingerprint);
        assert_eq!(
            first.schema_fingerprint,
            "sha256:0e991fd37a1fe8f81cb9bba505592d7d7f257c685f71c58a3abbd5ec54fdf890"
        );
        assert_eq!(
            first.protocol_fingerprint,
            "sha256:50a3690689ff5aa7cefc88bb7b5d6f1e1a64615e7644d306403287c09b1e59dc"
        );

        let user = first
            .models
            .iter()
            .find(|model| model.id == "UserView")
            .unwrap();
        assert_eq!(user.normalization, ModelNormalization::Embedded);
        assert!(user.record_revisions && user.tombstones);
        assert_eq!(
            user.fields
                .iter()
                .map(|field| field.name.as_str())
                .collect::<Vec<_>>(),
            vec!["display_name"]
        );
        let todo = first
            .models
            .iter()
            .find(|model| model.id == "TodoView")
            .unwrap();
        assert!(todo.record_revisions && todo.tombstones);
        assert_eq!(todo.row_policy, ClientRowPolicy::ServerOnly);
        let owner = todo
            .relationships
            .iter()
            .find(|rel| rel.name == "owner")
            .unwrap();
        assert!(!owner.nullable);
        assert_eq!(owner.key_mapping, RelationshipKeyMapping::Embedded);
        assert_eq!(owner.maintenance, ClientRelationshipMaintenance::Revalidate);
        assert_eq!(owner.dependencies, vec!["todos", "users"]);
        assert!(
            !owner.live,
            "singular relationships are not live list plans"
        );
        assert!(owner.arguments.is_empty());
        assert!(owner.filter.is_none());
        assert_eq!(todo.filter_input.type_name, "todos_bool_exp");
        assert_eq!(
            todo.filter_input
                .relationships
                .iter()
                .find(|relationship| relationship.field == "owner")
                .expect("singular relationship predicate")
                .target_type,
            user.filter_input.type_name
        );
        assert_eq!(user.filter_input.type_name, "users_bool_exp");
        assert!(first.capabilities.live_queries);
        assert!(!first.capabilities.record_revisions);
        assert!(!first.capabilities.tombstones);
        assert!(!first.capabilities.live_resume);
        assert_eq!(first.capabilities.query_fallback, "revalidate");
        assert!(first.capabilities.causal_receipts);
        assert!(first.capabilities.cache_scope);
        let membership = first
            .models
            .iter()
            .find(|model| model.id == "MembershipView")
            .unwrap();
        assert!(!membership.record_revisions && !membership.tombstones);
        let status = first
            .protocol_operations
            .command_status
            .as_ref()
            .expect("causal surfaces generate the framework status operation");
        assert_eq!(status.name, "Distributed_CommandStatus");
        assert_eq!(status.operation, command_status_operation());
        assert_eq!(
            status.operation_hash,
            hash_bytes(status.operation.as_bytes())
        );
        assert!(first
            .commands
            .iter()
            .any(|command| command.name == "todo.complete"));
        assert!(!first
            .commands
            .iter()
            .any(|command| command.name == "todo.force_archive"));
        assert_eq!(first.commands[0].grants, vec!["user"]);
        assert!(first.commands.iter().all(|command| {
            command
                .extensions
                .consistency
                .as_ref()
                .is_some_and(|consistency| consistency.kind == "accepted")
                && command.extensions.effects.as_ref().is_some_and(|effects| {
                    effects.operations.is_empty() && effects.fallback == "revalidate"
                })
                && command.extensions.confirmations.is_none()
        }));

        let json = serde_json::to_string(&first).unwrap();
        assert!(!json.contains("secret"));
        assert!(!json.contains("user_id"));
        assert!(!json.contains("force_archive"));
        assert!(!json.contains("x-user-id"));
    }

    #[test]
    fn filter_execution_limits_are_schema_fingerprinted_without_changing_protocol_epoch() {
        let full = full_surface();
        let selected = surface_for_role(&full, "user", &grants()["user"]).unwrap();
        let baseline =
            DistributedClientSurfaceExport::from_selected("todos-service", selected.clone())
                .unwrap()
                .manifest()
                .unwrap();

        let mut bool_limits = ClientExecutionLimits::default();
        bool_limits.max_bool_width += 1;
        let bool_manifest = DistributedClientSurfaceExport::from_selected_with_execution(
            "todos-service",
            selected.clone(),
            bool_limits,
        )
        .unwrap()
        .manifest()
        .unwrap();

        let mut in_limits = ClientExecutionLimits::default();
        in_limits.max_in_list += 1;
        let in_manifest = DistributedClientSurfaceExport::from_selected_with_execution(
            "todos-service",
            selected,
            in_limits,
        )
        .unwrap()
        .manifest()
        .unwrap();

        assert_ne!(
            baseline.schema_fingerprint,
            bool_manifest.schema_fingerprint
        );
        assert_ne!(baseline.schema_fingerprint, in_manifest.schema_fingerprint);
        assert_ne!(
            bool_manifest.schema_fingerprint,
            in_manifest.schema_fingerprint
        );
        assert_eq!(baseline.protocol_version, 2);
        assert_eq!(
            baseline.protocol_fingerprint,
            bool_manifest.protocol_fingerprint
        );
        assert_eq!(
            baseline.protocol_fingerprint,
            in_manifest.protocol_fingerprint
        );
    }

    #[test]
    fn relationship_nullability_is_copied_from_the_authoritative_surface() {
        let mut fingerprints = Vec::new();
        for nullable in [false, true] {
            let mut todo_schema = todos();
            todo_schema
                .columns
                .iter_mut()
                .find(|column| column.column_name == "owner_id")
                .expect("owner foreign key")
                .nullable = nullable;
            let full = build_surface(&[todo_schema, users()], &SurfaceOptions::sqlite()).unwrap();
            let selected = surface_for_role(
                &full,
                "user",
                &BTreeMap::from([
                    ("TodoView".into(), RoleGrant::all_columns()),
                    ("UserView".into(), RoleGrant::all_columns()),
                ]),
            )
            .unwrap();
            let surface_nullable = selected.models["TodoView"]
                .relationships
                .iter()
                .find(|relationship| relationship.name == "owner")
                .expect("surface relationship")
                .nullable;
            assert_eq!(surface_nullable, nullable);

            let manifest = client_manifest_from_surface(
                "todos-service",
                ClientSurfaceIdentity::role("user"),
                &selected,
            )
            .unwrap();
            fingerprints.push(manifest.schema_fingerprint.clone());
            let owner = manifest
                .models
                .iter()
                .find(|model| model.id == "TodoView")
                .unwrap()
                .relationships
                .iter()
                .find(|relationship| relationship.name == "owner")
                .unwrap();
            assert_eq!(owner.nullable, surface_nullable);
            assert_eq!(serde_json::to_value(owner).unwrap()["nullable"], nullable);
        }
        assert_ne!(
            fingerprints[0], fingerprints[1],
            "relationship nullability is part of the schema fingerprint"
        );
    }

    #[test]
    fn query_protocol_capabilities_are_complete_or_explicitly_revalidate() {
        let fully_owned = build_surface(&[todos(), users()], &SurfaceOptions::sqlite())
            .unwrap()
            .with_projectors([
                SurfaceProjector::new("project_todos")
                    .facts(["todo.changed"])
                    .models(["TodoView"])
                    .change_epoch("todos-v1"),
                SurfaceProjector::new("project_users")
                    .facts(["user.changed"])
                    .models(["UserView"])
                    .partition_constant(serde_json::json!({"scope": "all"}))
                    .change_epoch("users-v1"),
            ])
            .unwrap();
        let fully_owned = manifest_for_all_models("query-capabilities", "user", &fully_owned);
        assert!(fully_owned.capabilities.record_revisions);
        assert!(fully_owned.capabilities.tombstones);
        assert!(fully_owned.capabilities.live_resume);
        assert_eq!(fully_owned.capabilities.query_fallback, "revalidate");
        assert!(fully_owned
            .models
            .iter()
            .all(|model| model.record_revisions && model.tombstones));
        let wire = serde_json::to_value(&fully_owned).unwrap();
        assert_eq!(wire["capabilities"]["record_revisions"], true);
        assert_eq!(wire["capabilities"]["query_fallback"], "revalidate");
        assert!(wire["capabilities"].get("framework_revisions").is_none());
        assert!(wire["models"][0].get("framework_revision").is_none());

        let dynamic = build_surface(&[users()], &SurfaceOptions::sqlite())
            .unwrap()
            .with_projectors([SurfaceProjector::new("project_users")
                .facts(["user.changed"])
                .models(["UserView"])
                .partition_by(["tenant_id"])
                .change_epoch("users-v1")])
            .unwrap();
        let dynamic = manifest_for_all_models("query-capabilities", "user", &dynamic);
        assert!(dynamic.capabilities.record_revisions);
        assert!(dynamic.capabilities.tombstones);
        assert!(!dynamic.capabilities.live_resume);
        assert_eq!(dynamic.capabilities.query_fallback, "revalidate");

        let row_filtered = build_surface(&[users()], &SurfaceOptions::sqlite())
            .unwrap()
            .with_projectors([SurfaceProjector::new("project_users")
                .facts(["user.changed"])
                .models(["UserView"])
                .change_epoch("users-v1")])
            .unwrap();
        let row_filtered = surface_for_role(
            &row_filtered,
            "user",
            &BTreeMap::from([(
                "UserView".into(),
                RoleGrant::all_columns().rows(col("user_id").eq("visible-user")),
            )]),
        )
        .unwrap();
        let row_filtered = client_manifest_from_surface(
            "query-capabilities",
            ClientSurfaceIdentity::role("user"),
            &row_filtered,
        )
        .unwrap();
        assert!(row_filtered.capabilities.record_revisions);
        assert!(row_filtered.capabilities.tombstones);
        assert!(
            !row_filtered.capabilities.live_resume,
            "partition-wide positions and changes must not cross a row-authorization boundary"
        );

        let epochless = build_surface(&[users()], &SurfaceOptions::sqlite())
            .unwrap()
            .with_projectors([SurfaceProjector::new("project_users")
                .facts(["user.changed"])
                .models(["UserView"])])
            .unwrap();
        let epochless = manifest_for_all_models("query-capabilities", "user", &epochless);
        assert!(epochless.capabilities.record_revisions);
        assert!(epochless.capabilities.tombstones);
        assert!(!epochless.capabilities.live_resume);
        assert_eq!(epochless.capabilities.query_fallback, "revalidate");

        let unowned = build_surface(&[users()], &SurfaceOptions::sqlite()).unwrap();
        let unowned = manifest_for_all_models("query-capabilities", "user", &unowned);
        assert!(!unowned.capabilities.record_revisions);
        assert!(!unowned.capabilities.tombstones);
        assert!(!unowned.capabilities.live_resume);
        assert_eq!(unowned.capabilities.query_fallback, "revalidate");
        assert!(unowned
            .models
            .iter()
            .all(|model| !model.record_revisions && !model.tombstones));

        let mixed = build_surface(&[todos(), users()], &SurfaceOptions::sqlite())
            .unwrap()
            .with_projectors([SurfaceProjector::new("project_todos")
                .facts(["todo.changed"])
                .models(["TodoView"])
                .change_epoch("todos-v1")])
            .unwrap();
        let mixed = manifest_for_all_models("query-capabilities", "user", &mixed);
        assert!(!mixed.capabilities.record_revisions);
        assert!(!mixed.capabilities.tombstones);
        assert!(!mixed.capabilities.live_resume);
        assert_eq!(mixed.capabilities.query_fallback, "revalidate");
        assert!(mixed
            .models
            .iter()
            .find(|model| model.id == "TodoView")
            .is_some_and(|model| model.record_revisions && model.tombstones));
        assert!(mixed
            .models
            .iter()
            .find(|model| model.id == "UserView")
            .is_some_and(|model| !model.record_revisions && !model.tombstones));

        let uncovered_join = build_surface(
            &[teams(), users(), team_members()],
            &SurfaceOptions::sqlite(),
        )
        .unwrap()
        .with_projectors([
            SurfaceProjector::new("project_teams")
                .facts(["team.changed"])
                .models(["TeamView"])
                .change_epoch("teams-v1"),
            SurfaceProjector::new("project_users")
                .facts(["user.changed"])
                .models(["UserView"])
                .change_epoch("users-v1"),
        ])
        .unwrap();
        let uncovered_join = manifest_for_all_models("query-capabilities", "user", &uncovered_join);
        assert!(uncovered_join.capabilities.record_revisions);
        assert!(uncovered_join.capabilities.tombstones);
        assert!(!uncovered_join.capabilities.live_resume);
        assert_eq!(uncovered_join.capabilities.query_fallback, "revalidate");

        let mut query_only_options = SurfaceOptions::sqlite();
        query_only_options.subscriptions = false;
        let query_only = build_surface(&[users()], &query_only_options)
            .unwrap()
            .with_projectors([SurfaceProjector::new("project_users")
                .facts(["user.changed"])
                .models(["UserView"])
                .change_epoch("users-v1")])
            .unwrap();
        let query_only = manifest_for_all_models("query-capabilities", "user", &query_only);
        assert!(query_only.capabilities.record_revisions);
        assert!(query_only.capabilities.tombstones);
        assert!(!query_only.capabilities.live_queries);
        assert!(!query_only.capabilities.live_resume);
        assert_eq!(query_only.capabilities.query_fallback, "revalidate");

        let empty = build_surface(&[], &SurfaceOptions::sqlite()).unwrap();
        let empty = manifest_for_all_models("query-capabilities", "user", &empty);
        assert!(!empty.capabilities.record_revisions);
        assert!(!empty.capabilities.tombstones);
        assert!(!empty.capabilities.live_resume);
        assert_eq!(empty.capabilities.query_fallback, "revalidate");
    }

    #[test]
    fn composite_keys_normalize_in_declared_order_and_hidden_keys_embed() {
        let full = full_surface();
        let admin = surface_for_role(&full, "admin", &grants()["admin"]).unwrap();
        let admin_manifest = client_manifest_from_surface(
            "todos-service",
            ClientSurfaceIdentity::role("admin"),
            &admin,
        )
        .unwrap();
        let membership = admin_manifest
            .models
            .iter()
            .find(|model| model.id == "MembershipView")
            .unwrap();
        let ModelNormalization::Normalized { fields, encoding } = &membership.normalization else {
            panic!("composite model should normalize")
        };
        assert_eq!(
            fields
                .iter()
                .map(|field| field.name.as_str())
                .collect::<Vec<_>>(),
            vec!["tenant_id", "user_id"]
        );
        assert_eq!(encoding, KEY_ENCODING);

        let user = surface_for_role(&full, "user", &grants()["user"]).unwrap();
        let user_manifest = client_manifest_from_surface(
            "todos-service",
            ClientSurfaceIdentity::role("user"),
            &user,
        )
        .unwrap();
        let membership = user_manifest
            .models
            .iter()
            .find(|model| model.id == "MembershipView")
            .unwrap();
        assert_eq!(membership.normalization, ModelNormalization::Embedded);
        assert!(!serde_json::to_string(membership)
            .unwrap()
            .contains("user_id"));
    }

    #[test]
    fn bigint_keys_embed_until_decimal_string_identity_is_available() {
        let accounts = TableSchema {
            model_name: "AccountView".into(),
            table_name: "accounts".into(),
            columns: vec![TableColumn {
                primary_key: true,
                ..column("account_id", ColumnType::Integer)
            }],
            primary_key: PrimaryKey::new(["account_id"]),
            version_column: None,
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships: Vec::new(),
            kind: TableKind::ReadModel,
        };
        let invoices = TableSchema {
            model_name: "InvoiceView".into(),
            table_name: "invoices".into(),
            columns: vec![
                TableColumn {
                    primary_key: true,
                    ..column("invoice_id", ColumnType::Text)
                },
                column("account_id", ColumnType::Integer),
            ],
            primary_key: PrimaryKey::new(["invoice_id"]),
            version_column: None,
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships: vec![RelationshipDef {
                field_name: "account".into(),
                kind: RelationshipKind::BelongsTo,
                target_model: "AccountView".into(),
                foreign_key: Some("account_id".into()),
                through: None,
                target_foreign_key: None,
            }],
            kind: TableKind::ReadModel,
        };
        let full = build_surface(&[accounts, invoices], &SurfaceOptions::sqlite()).unwrap();
        let selected = surface_for_role(
            &full,
            "user",
            &BTreeMap::from([
                (
                    "AccountView".into(),
                    RoleGrant::all_columns().rows(col("account_id").eq(9_007_199_254_740_992_i64)),
                ),
                ("InvoiceView".into(), RoleGrant::all_columns()),
            ]),
        )
        .unwrap();
        let manifest = DistributedClientSurfaceExport::from_selected("billing", selected)
            .unwrap()
            .manifest()
            .unwrap();

        let account = manifest
            .models
            .iter()
            .find(|model| model.id == "AccountView")
            .unwrap();
        assert_eq!(account.normalization, ModelNormalization::Embedded);
        assert_eq!(account.row_policy, ClientRowPolicy::ServerOnly);
        assert!(!serde_json::to_string(account)
            .unwrap()
            .contains("9007199254740992"));
        let relationship = manifest
            .models
            .iter()
            .find(|model| model.id == "InvoiceView")
            .unwrap()
            .relationships
            .iter()
            .find(|relationship| relationship.name == "account")
            .unwrap();
        assert_eq!(relationship.key_mapping, RelationshipKeyMapping::Embedded);
        assert_eq!(
            relationship.maintenance,
            ClientRelationshipMaintenance::Revalidate
        );
    }

    #[test]
    fn application_surface_is_common_contract_with_safe_role_limit_semantics() {
        let full = full_surface();
        let all_grants = grants();
        let selected =
            surface_for_application(&full, "web", &["user".into(), "admin".into()], &all_grants)
                .unwrap();
        let manifest = client_manifest_from_surface(
            "todos-service",
            ClientSurfaceIdentity::application("web", ["admin", "user"]),
            &selected,
        )
        .unwrap();
        assert!(!manifest
            .commands
            .iter()
            .any(|command| command.name == "todo.force_archive"));
        let todos = manifest
            .roots
            .iter()
            .find(|root| root.id == "query:todos")
            .unwrap();
        assert_eq!(todos.pagination.as_ref().unwrap().default_limit, 25);
        assert_eq!(todos.pagination.as_ref().unwrap().max_limit, 25);
        assert!(matches!(
            todos.filter.as_ref().unwrap().row_policy,
            ClientRowPolicy::ServerOnly
        ));

        let admin = surface_for_role(&full, "admin", &all_grants["admin"]).unwrap();
        let admin_manifest = client_manifest_from_surface(
            "todos-service",
            ClientSurfaceIdentity::role("admin"),
            &admin,
        )
        .unwrap();
        let admin_todos = admin_manifest
            .roots
            .iter()
            .find(|root| root.id == "query:todos")
            .unwrap();
        assert_eq!(admin_todos.pagination.as_ref().unwrap().default_limit, 100);
        assert_eq!(admin_todos.pagination.as_ref().unwrap().max_limit, 1000);
        assert_ne!(
            manifest.schema_fingerprint,
            admin_manifest.schema_fingerprint
        );
        assert_eq!(
            manifest.protocol_fingerprint,
            admin_manifest.protocol_fingerprint
        );
    }

    #[test]
    fn mixed_target_projectors_are_omitted_for_role_and_application_surfaces() {
        let full = build_surface(&[todos(), users(), teams()], &SurfaceOptions::sqlite())
            .unwrap()
            .with_projectors([
                SurfaceProjector::new("project_todos")
                    .facts(["todo.changed"])
                    .models(["TodoView"]),
                SurfaceProjector::new("project_user_team")
                    .facts(["private-user.changed"])
                    .models(["UserView", "TeamView"]),
            ])
            .unwrap();
        let restricted = BTreeMap::from([
            ("TodoView".into(), RoleGrant::all_columns()),
            ("UserView".into(), RoleGrant::all_columns()),
        ]);
        let admin = BTreeMap::from([
            ("TodoView".into(), RoleGrant::all_columns()),
            ("UserView".into(), RoleGrant::all_columns()),
            ("TeamView".into(), RoleGrant::all_columns()),
        ]);

        let role = surface_for_role(&full, "restricted", &restricted).unwrap();
        let role_manifest = DistributedClientSurfaceExport::from_selected("todos-service", role)
            .unwrap()
            .manifest()
            .unwrap();
        assert_eq!(
            role_manifest
                .projectors
                .iter()
                .map(|projector| projector.name.as_str())
                .collect::<Vec<_>>(),
            vec!["project_todos"]
        );
        assert!(!role_manifest.capabilities.record_revisions);
        assert!(!role_manifest.capabilities.tombstones);
        assert!(!role_manifest.capabilities.live_resume);
        assert!(role_manifest
            .models
            .iter()
            .find(|model| model.id == "TodoView")
            .is_some_and(|model| model.record_revisions && model.tombstones));
        assert!(role_manifest
            .models
            .iter()
            .find(|model| model.id == "UserView")
            .is_some_and(|model| !model.record_revisions && !model.tombstones));
        let role_json = serde_json::to_string(&role_manifest).unwrap();
        assert!(!role_json.contains("project_user_team"));
        assert!(!role_json.contains("private-user.changed"));

        let application = surface_for_application(
            &full,
            "web",
            &["admin".into(), "restricted".into()],
            &BTreeMap::from([("admin".into(), admin), ("restricted".into(), restricted)]),
        )
        .unwrap();
        let application_manifest =
            DistributedClientSurfaceExport::from_selected("todos-service", application)
                .unwrap()
                .manifest()
                .unwrap();
        assert_eq!(
            application_manifest
                .projectors
                .iter()
                .map(|projector| projector.name.as_str())
                .collect::<Vec<_>>(),
            vec!["project_todos"]
        );
        assert!(!application_manifest.capabilities.record_revisions);
        assert!(!application_manifest.capabilities.tombstones);
        assert!(!application_manifest.capabilities.live_resume);
        let application_json = serde_json::to_string(&application_manifest).unwrap();
        assert!(!application_json.contains("project_user_team"));
        assert!(!application_json.contains("private-user.changed"));
    }

    #[test]
    fn aggregate_nodes_export_exact_role_bounded_window_semantics() {
        let mut options = SurfaceOptions::sqlite();
        options.default_limit = 7;
        options.max_limit = 19;
        let full = build_surface(&[teams(), users(), team_members()], &options).unwrap();
        let selected = surface_for_role(
            &full,
            "admin",
            &BTreeMap::from([
                ("TeamView".into(), RoleGrant::all_columns()),
                (
                    "UserView".into(),
                    RoleGrant::all_columns().with_aggregations().limit(13),
                ),
            ]),
        )
        .unwrap();
        let manifest = client_manifest_from_surface(
            "teams-service",
            ClientSurfaceIdentity::role("admin"),
            &selected,
        )
        .unwrap();

        let users_aggregate = manifest
            .roots
            .iter()
            .find(|root| {
                root.operation == ClientRootOperation::Query && root.name == "users_aggregate"
            })
            .expect("aggregate root grant");
        assert!(
            users_aggregate.pagination.is_none(),
            "ordinary root pagination must not stand in for aggregate nodes"
        );
        let root_semantics = users_aggregate
            .aggregate
            .as_ref()
            .expect("aggregate root semantics");
        assert_eq!(root_semantics.wrapper_typename, "users_aggregate");
        assert_eq!(root_semantics.fields_typename, "users_aggregate_fields");
        assert_eq!(
            root_semantics.nodes_pagination,
            ClientPaginationSemantics {
                kind: "offset".into(),
                default_limit: 7,
                max_limit: 13,
                coverage: "window".into(),
            }
        );

        let members_aggregate = manifest
            .models
            .iter()
            .find(|model| model.id == "TeamView")
            .unwrap()
            .relationships
            .iter()
            .find(|relationship| relationship.name == "members")
            .unwrap()
            .aggregate
            .as_ref()
            .expect("relationship aggregate grant");
        assert_eq!(
            members_aggregate.semantics.wrapper_typename,
            "users_aggregate"
        );
        assert_eq!(
            members_aggregate.semantics.fields_typename,
            "users_aggregate_fields"
        );
        assert_eq!(
            members_aggregate.semantics.nodes_pagination,
            root_semantics.nodes_pagination
        );
        assert_eq!(
            serde_json::to_value(&members_aggregate.semantics).unwrap()["nodes_pagination"],
            serde_json::json!({
                "kind": "offset",
                "default_limit": 7,
                "max_limit": 13,
                "coverage": "window"
            })
        );
    }

    #[test]
    fn opaque_m2m_plan_preserves_invalidation_without_leaking_join_internals() {
        let full = build_surface(
            &[teams(), users(), team_members()],
            &SurfaceOptions::sqlite(),
        )
        .unwrap();
        let admin = surface_for_role(
            &full,
            "admin",
            &BTreeMap::from([
                ("TeamView".into(), RoleGrant::all_columns()),
                (
                    "UserView".into(),
                    RoleGrant::all_columns().with_aggregations(),
                ),
            ]),
        )
        .unwrap();
        let manifest = client_manifest_from_surface(
            "teams-service",
            ClientSurfaceIdentity::role("admin"),
            &admin,
        )
        .unwrap();
        assert!(!manifest.capabilities.causal_receipts);
        assert!(!manifest.capabilities.cache_scope);
        assert!(manifest.protocol_operations.command_status.is_none());
        let members = manifest
            .models
            .iter()
            .find(|model| model.id == "TeamView")
            .unwrap()
            .relationships
            .iter()
            .find(|relationship| relationship.name == "members")
            .unwrap();
        assert!(!members.nullable, "list relationships are non-null lists");
        let RelationshipKeyMapping::ThroughOpaque {
            local,
            remote,
            dependency,
        } = &members.key_mapping
        else {
            panic!("authorized source/target keys should retain an opaque m2m plan")
        };
        assert_eq!(local, &["team_id"]);
        assert_eq!(remote, &["user_id"]);
        assert!(dependency.starts_with("opaque:sha256:"));
        assert_eq!(
            members.maintenance,
            ClientRelationshipMaintenance::Revalidate
        );
        let aggregate = members.aggregate.as_ref().expect("aggregate grant");
        assert_eq!(aggregate.name, "members_aggregate");
        assert_eq!(aggregate.semantics.wrapper_typename, "users_aggregate");
        assert_eq!(
            aggregate.semantics.fields_typename,
            "users_aggregate_fields"
        );
        assert!(aggregate.semantics.count && aggregate.semantics.nodes);
        assert_eq!(aggregate.dependencies, members.dependencies);
        let users_aggregate = manifest
            .roots
            .iter()
            .find(|root| {
                root.operation == ClientRootOperation::Query && root.name == "users_aggregate"
            })
            .expect("aggregate root grant");
        let users_aggregate_semantics = users_aggregate
            .aggregate
            .as_ref()
            .expect("aggregate root semantics");
        assert_eq!(
            users_aggregate_semantics.wrapper_typename,
            "users_aggregate"
        );
        assert_eq!(
            users_aggregate_semantics.fields_typename,
            "users_aggregate_fields"
        );
        assert!(members.dependencies.contains(&"teams".into()));
        assert!(members.dependencies.contains(&"users".into()));
        assert!(members
            .dependencies
            .iter()
            .any(|dependency| dependency.starts_with("opaque:sha256:")));
        let json = serde_json::to_string(&manifest).unwrap();
        assert!(!json.contains("private_team_members"));

        let mut renamed_team = teams();
        renamed_team.relationships[0].through = Some("renamed_private_join".into());
        let mut renamed_join = team_members();
        renamed_join.table_name = "renamed_private_join".into();
        let renamed_full = build_surface(
            &[renamed_team, users(), renamed_join],
            &SurfaceOptions::sqlite(),
        )
        .unwrap();
        let renamed_admin = surface_for_role(
            &renamed_full,
            "admin",
            &BTreeMap::from([
                ("TeamView".into(), RoleGrant::all_columns()),
                (
                    "UserView".into(),
                    RoleGrant::all_columns().with_aggregations(),
                ),
            ]),
        )
        .unwrap();
        let renamed_manifest = client_manifest_from_surface(
            "teams-service",
            ClientSurfaceIdentity::role("admin"),
            &renamed_admin,
        )
        .unwrap();
        let renamed_members = renamed_manifest
            .models
            .iter()
            .find(|model| model.id == "TeamView")
            .unwrap()
            .relationships
            .iter()
            .find(|relationship| relationship.name == "members")
            .unwrap();
        let RelationshipKeyMapping::ThroughOpaque {
            dependency: renamed_dependency,
            ..
        } = &renamed_members.key_mapping
        else {
            panic!("renamed private join should remain opaque")
        };
        assert_eq!(renamed_dependency, dependency);

        let denied = surface_for_role(
            &full,
            "limited",
            &BTreeMap::from([
                ("TeamView".into(), RoleGrant::columns(["name"])),
                ("UserView".into(), RoleGrant::all_columns()),
            ]),
        )
        .unwrap();
        let denied = client_manifest_from_surface(
            "teams-service",
            ClientSurfaceIdentity::role("limited"),
            &denied,
        )
        .unwrap();
        let team = denied
            .models
            .iter()
            .find(|model| model.id == "TeamView")
            .unwrap();
        let members = team
            .relationships
            .iter()
            .find(|relationship| relationship.name == "members")
            .unwrap();
        assert_eq!(members.key_mapping, RelationshipKeyMapping::Embedded);
        assert_eq!(
            members.maintenance,
            ClientRelationshipMaintenance::Revalidate
        );
        assert!(!members.dependencies.is_empty());
        assert!(members.aggregate.is_none());
        let team_json = serde_json::to_string(team).unwrap();
        assert!(!team_json.contains("team_id"));
        assert!(!team_json.contains("private_team_members"));
    }

    #[test]
    fn visible_read_model_join_emits_explicit_local_m2m_plan() {
        let mut join = team_members();
        join.model_name = "TeamMemberView".into();
        join.kind = TableKind::ReadModel;
        let full = build_surface(&[teams(), users(), join], &SurfaceOptions::sqlite()).unwrap();
        let selected = surface_for_role(
            &full,
            "admin",
            &BTreeMap::from([
                ("TeamView".into(), RoleGrant::all_columns()),
                ("UserView".into(), RoleGrant::all_columns()),
                ("TeamMemberView".into(), RoleGrant::all_columns()),
            ]),
        )
        .unwrap();
        let manifest = DistributedClientSurfaceExport::from_selected("teams-service", selected)
            .unwrap()
            .manifest()
            .unwrap();
        let members = manifest
            .models
            .iter()
            .find(|model| model.id == "TeamView")
            .unwrap()
            .relationships
            .iter()
            .find(|relationship| relationship.name == "members")
            .unwrap();
        assert_eq!(
            members.key_mapping,
            RelationshipKeyMapping::Through {
                local: vec!["team_id".into()],
                remote: vec!["user_id".into()],
                table: "private_team_members".into(),
                source_foreign_key: "team_id".into(),
                target_foreign_key: "user_id".into(),
            }
        );
        assert_eq!(members.maintenance, ClientRelationshipMaintenance::Local);
        assert_eq!(
            members.dependencies,
            vec!["private_team_members", "teams", "users"]
        );
    }

    #[test]
    fn relational_row_policy_is_server_only_when_relationship_key_is_hidden() {
        let full = build_surface(&[todos(), users()], &SurfaceOptions::sqlite()).unwrap();
        let selected = surface_for_role(
            &full,
            "user",
            &BTreeMap::from([
                (
                    "TodoView".into(),
                    RoleGrant::columns(["todo_id", "title", "completed"])
                        .rows(rel("owner", col("display_name").eq("Patrick"))),
                ),
                ("UserView".into(), RoleGrant::columns(["display_name"])),
            ]),
        )
        .unwrap();
        let manifest = client_manifest_from_surface(
            "todos-service",
            ClientSurfaceIdentity::role("user"),
            &selected,
        )
        .unwrap();
        let todo = manifest
            .models
            .iter()
            .find(|model| model.id == "TodoView")
            .unwrap();
        assert_eq!(todo.row_policy, ClientRowPolicy::ServerOnly);
        let owner = todo
            .relationships
            .iter()
            .find(|relationship| relationship.name == "owner")
            .unwrap();
        assert_eq!(owner.key_mapping, RelationshipKeyMapping::Embedded);
        assert_eq!(owner.dependencies, vec!["todos", "users"]);
        assert_eq!(owner.maintenance, ClientRelationshipMaintenance::Revalidate);
        let json = serde_json::to_string(todo).unwrap();
        assert!(!json.contains("owner_id"));
        assert!(!json.contains("user_id"));
        assert!(!json.contains("Patrick"));
    }

    #[test]
    fn application_role_sets_are_canonical_before_fingerprinting() {
        let full = full_surface();
        let selected =
            surface_for_application(&full, "web", &["admin".into(), "user".into()], &grants())
                .unwrap();
        let first = client_manifest_from_surface(
            "todos-service",
            ClientSurfaceIdentity::Application {
                name: "web".into(),
                roles: vec!["user".into(), "admin".into(), "user".into()],
            },
            &selected,
        )
        .unwrap();
        let second = client_manifest_from_surface(
            "todos-service",
            ClientSurfaceIdentity::Application {
                name: "web".into(),
                roles: vec!["admin".into(), "user".into()],
            },
            &selected,
        )
        .unwrap();
        assert_eq!(first, second);
        assert_eq!(first.schema_fingerprint, second.schema_fingerprint);
        assert_eq!(
            first.surface,
            ClientSurfaceIdentity::application("web", ["admin", "user"])
        );
    }

    #[test]
    fn catalog_or_mismatched_surface_cannot_be_labeled_as_authorized() {
        let full = full_surface();
        let error = DistributedClientSurfaceExport::from_selected("todos-service", full.clone())
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("explicitly role- or application-selected"));

        let selected = surface_for_role(&full, "user", &grants()["user"]).unwrap();
        let wrong_project = DistributedProjectManifest::new("wrong-service").table_schema(users());
        let inventory_error =
            DistributedClientSurfaceExport::from_project(&wrong_project, selected.clone())
                .unwrap_err();
        assert!(inventory_error.to_string().contains("does not match"));

        let error = client_manifest_from_surface(
            "todos-service",
            ClientSurfaceIdentity::role("admin"),
            &selected,
        )
        .unwrap_err();
        assert!(error.to_string().contains("does not match"));
    }
}
