use super::*;

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
    /// Durable restore of confirmed normalized state.
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

/// Versioned typed command semantics exported from the executable service.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientCommandExtensionSlots {
    pub version: u32,
    pub consistency: CommandConsistencyExtension,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direct_projection: Option<CommandDirectProjectionExtension>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_defaults: Option<CommandInputDefaultsExtension>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effects: Option<CommandEffectsExtension>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmations: Option<CommandConfirmationsExtension>,
    /// Names and wire codecs only. Values are server-derived for the current
    /// verified Session and are never frozen into a generated artifact.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trusted_presets: Vec<ClientTrustedPresetDescriptor>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ClientTrustedPresetDescriptor {
    pub name: String,
    pub codec: String,
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
