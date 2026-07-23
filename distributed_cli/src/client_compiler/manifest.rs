use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};

use super::{ClientCompileError, ClientSurfaceSelector};

const MANIFEST_VERSION: u64 = 7;
const PROTOCOL_VERSION: u64 = 2;
const PROTOCOL_FINGERPRINT: &str =
    "sha256:a3b12d91f7d60ab279cfffe6bb708852b6e9f6641d6aa0311cce2103600ccdc3";

#[derive(Clone, Debug)]
pub(crate) struct ClientManifest {
    pub(crate) service_id: String,
    pub(crate) surface: ManifestSurface,
    pub(crate) schema_fingerprint: String,
    pub(crate) protocol_fingerprint: String,
    pub(crate) execution: ManifestExecutionLimits,
    pub(crate) capabilities: ManifestCapabilities,
    pub(crate) scalar_codecs: BTreeMap<String, String>,
    pub(crate) models: BTreeMap<String, ManifestModel>,
    pub(crate) roots: BTreeMap<(RootOperation, String), ManifestRoot>,
    pub(crate) commands: Vec<ManifestCommand>,
    pub(crate) commands_requiring_revalidation: BTreeSet<String>,
    pub(crate) protocol_operations: ManifestProtocolOperations,
    pub(crate) projectors: Vec<ManifestProjector>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestExecutionLimits {
    pub(crate) max_depth: u64,
    pub(crate) max_complexity: u64,
    pub(crate) max_bool_width: u64,
    pub(crate) max_in_list: u64,
    pub(crate) complexity: ManifestComplexityWeights,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestComplexityWeights {
    pub(crate) version: u32,
    pub(crate) scalar: u64,
    pub(crate) belongs_to: u64,
    pub(crate) has_many: u64,
    pub(crate) m2m: u64,
    pub(crate) aggregate: u64,
    pub(crate) list_root: u64,
    pub(crate) by_pk: u64,
    pub(crate) list_fanout: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ManifestSurface {
    Role { name: String },
    Application { name: String, roles: Vec<String> },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestCapabilities {
    pub(crate) live_queries: bool,
    pub(crate) record_revisions: bool,
    pub(crate) tombstones: bool,
    pub(crate) causal_receipts: bool,
    pub(crate) live_resume: bool,
    pub(crate) query_fallback: String,
    pub(crate) cache_scope: bool,
    pub(crate) confirmed_persistence: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestScalarCodec {
    scalar: String,
    codec: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestModel {
    pub(crate) id: String,
    pub(crate) typename: String,
    pub(crate) source_table: String,
    pub(crate) dependencies: Vec<String>,
    pub(crate) normalization: ManifestNormalization,
    pub(crate) fields: Vec<ManifestField>,
    pub(crate) relationships: Vec<ManifestRelationship>,
    pub(crate) filter_input: ManifestFilterInput,
    pub(crate) row_policy: ManifestRowPolicy,
    pub(crate) record_revisions: bool,
    pub(crate) tombstones: bool,
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ManifestNormalization {
    Normalized {
        fields: Vec<ManifestKeyField>,
        encoding: String,
    },
    Embedded,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestKeyField {
    pub(crate) name: String,
    pub(crate) codec: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestField {
    pub(crate) name: String,
    pub(crate) scalar: String,
    pub(crate) codec: String,
    pub(crate) nullable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestFilterInput {
    pub(crate) type_name: String,
    pub(crate) fields: Vec<ManifestFilterField>,
    pub(crate) relationships: Vec<ManifestFilterInputRelationship>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestFilterInputRelationship {
    pub(crate) field: String,
    pub(crate) target_type: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ManifestRowPolicy {
    Unrestricted,
    Predicate { expression: ManifestFilterExpr },
    ServerOnly,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub(crate) enum ManifestFilterExpr {
    And(Vec<ManifestFilterExpr>),
    Or(Vec<ManifestFilterExpr>),
    Not(Box<ManifestFilterExpr>),
    Cmp {
        column: String,
        op: ManifestCmpOp,
        rhs: ManifestOperand,
    },
    In {
        column: String,
        values: Vec<ManifestOperand>,
        negated: bool,
    },
    IsNull {
        column: String,
        is_null: bool,
    },
    Rel {
        field: String,
        predicate: Box<ManifestFilterExpr>,
    },
}

impl Serialize for ManifestFilterExpr {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde_json::json;

        fn wire_value(expression: &ManifestFilterExpr) -> JsonValue {
            match expression {
                ManifestFilterExpr::And(items) => json!({
                    "kind": "and",
                    "value": items.iter().map(wire_value).collect::<Vec<_>>()
                }),
                ManifestFilterExpr::Or(items) => json!({
                    "kind": "or",
                    "value": items.iter().map(wire_value).collect::<Vec<_>>()
                }),
                ManifestFilterExpr::Not(item) => {
                    json!({"kind": "not", "value": wire_value(item)})
                }
                ManifestFilterExpr::Cmp { column, op, rhs } => json!({
                    "kind": "cmp",
                    "value": {"column": column, "op": op, "rhs": rhs}
                }),
                ManifestFilterExpr::In {
                    column,
                    values,
                    negated,
                } => json!({
                    "kind": "in",
                    "value": {
                        "column": column,
                        "values": values,
                        "negated": negated
                    }
                }),
                ManifestFilterExpr::IsNull { column, is_null } => json!({
                    "kind": "is_null",
                    "value": {"column": column, "is_null": is_null}
                }),
                ManifestFilterExpr::Rel { field, predicate } => json!({
                    "kind": "rel",
                    "value": {"field": field, "predicate": wire_value(predicate)}
                }),
            }
        }

        wire_value(self).serialize(serializer)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub(crate) enum ManifestOperand {
    Lit(ManifestLitValue),
    Claim(ManifestClaimRef),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub(crate) enum ManifestLitValue {
    String(String),
    I64(i64),
    F64(f64),
    Bool(bool),
    Json(JsonValue),
    Null,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestClaimRef {
    pub(crate) header: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ManifestCmpOp {
    Eq,
    Neq,
    Gt,
    Gte,
    Lt,
    Lte,
    Like,
    Ilike,
    Contains,
    ContainedIn,
    HasKey,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestRelationship {
    pub(crate) name: String,
    pub(crate) target_model: String,
    pub(crate) target_typename: String,
    pub(crate) kind: ManifestRelationshipKind,
    pub(crate) list: bool,
    pub(crate) nullable: bool,
    pub(crate) arguments: Vec<ManifestArgument>,
    pub(crate) key_mapping: ManifestRelationshipKeyMapping,
    pub(crate) maintenance: ManifestRelationshipMaintenance,
    pub(crate) dependencies: Vec<String>,
    pub(crate) filter: Option<ManifestFilterSemantics>,
    pub(crate) order: Option<ManifestOrderSemantics>,
    pub(crate) pagination: Option<ManifestPagination>,
    pub(crate) aggregate: Option<ManifestRelationshipAggregate>,
    pub(crate) live: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ManifestRelationshipKind {
    HasMany,
    BelongsTo,
    ManyToMany,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ManifestRelationshipKeyMapping {
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
pub(crate) enum ManifestRelationshipMaintenance {
    Local,
    Revalidate,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestRelationshipAggregate {
    pub(crate) name: String,
    pub(crate) arguments: Vec<ManifestArgument>,
    pub(crate) semantics: ManifestAggregateSemantics,
    pub(crate) dependencies: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RootOperation {
    Query,
    Subscription,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RootKind {
    List,
    ByPk,
    Aggregate,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestRoot {
    pub(crate) id: String,
    pub(crate) operation: RootOperation,
    pub(crate) name: String,
    pub(crate) kind: RootKind,
    pub(crate) model: String,
    pub(crate) arguments: Vec<ManifestArgument>,
    pub(crate) filter: Option<ManifestFilterSemantics>,
    pub(crate) order: Option<ManifestOrderSemantics>,
    pub(crate) pagination: Option<ManifestPagination>,
    pub(crate) aggregate: Option<ManifestAggregateSemantics>,
    pub(crate) dependencies: Vec<String>,
    pub(crate) live: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestArgument {
    pub(crate) name: String,
    pub(crate) kind: ManifestArgumentKind,
    pub(crate) type_name: String,
    pub(crate) nullable: bool,
    pub(crate) list: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) codec: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestFilterSemantics {
    pub(crate) fields: Vec<ManifestFilterField>,
    pub(crate) relationships: Vec<String>,
    pub(crate) row_policy: ManifestRowPolicy,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestFilterField {
    pub(crate) name: String,
    pub(crate) operators: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestOrderSemantics {
    pub(crate) fields: Vec<String>,
    pub(crate) values: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestPagination {
    pub(crate) kind: String,
    pub(crate) default_limit: u64,
    pub(crate) max_limit: u64,
    pub(crate) coverage: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestAggregateSemantics {
    pub(crate) wrapper_typename: String,
    pub(crate) fields_typename: String,
    pub(crate) nodes_pagination: ManifestPagination,
    pub(crate) count: bool,
    pub(crate) nodes: bool,
    pub(crate) sum: Vec<String>,
    pub(crate) avg: Vec<String>,
    pub(crate) min: Vec<String>,
    pub(crate) max: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestCommand {
    pub(crate) version: u32,
    pub(crate) name: String,
    pub(crate) mutation_field: String,
    pub(crate) grants: Vec<String>,
    pub(crate) input: ManifestCommandShape,
    pub(crate) output: ManifestCommandShape,
    pub(crate) operation: String,
    pub(crate) operation_hash: String,
    pub(crate) extensions: ManifestCommandExtensions,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ManifestCommandShape {
    None,
    Json { codec: String },
    Object { definition: ManifestTypeDef },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestTypeDef {
    pub(crate) name: String,
    pub(crate) fields: Vec<ManifestTypeField>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestTypeField {
    pub(crate) name: String,
    pub(crate) type_name: String,
    pub(crate) nullable: bool,
    pub(crate) list: bool,
    pub(crate) item_nullable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) codec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) nested: Option<Box<ManifestTypeDef>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestCommandExtensions {
    pub(crate) version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) consistency: Option<ManifestCommandConsistency>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) direct_projection: Option<ManifestDirectProjection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) input_defaults: Option<ManifestInputDefaults>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) effects: Option<ManifestEffects>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) confirmations: Option<ManifestConfirmations>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) trusted_presets: Vec<ManifestTrustedPresetDescriptor>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestTrustedPresetDescriptor {
    pub(crate) name: String,
    pub(crate) codec: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestCommandConsistency {
    pub(crate) version: u32,
    pub(crate) kind: ManifestConsistencyKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestDirectProjection {
    pub(crate) topology: ManifestProjectionTopologyIdentity,
    pub(crate) model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) partition: Option<ManifestEffectExpression>,
    pub(crate) change_epoch: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestProjectionTopologyIdentity {
    pub(crate) version: u32,
    pub(crate) name: String,
    pub(crate) digest: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ManifestConsistencyKind {
    Accepted,
    Fact,
    Projected,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestInputDefaults {
    pub(crate) version: u32,
    pub(crate) defaults: Vec<ManifestInputDefault>,
}

impl Serialize for ManifestInputDefaults {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(Serialize)]
        struct Wire<'a> {
            version: u32,
            defaults: &'a [JsonValue],
        }

        let defaults = self
            .defaults
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .map_err(serde::ser::Error::custom)?;
        Wire {
            version: self.version,
            defaults: &defaults,
        }
        .serialize(serializer)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestInputDefault {
    pub(crate) path: Vec<String>,
    pub(crate) generator: ManifestInputDefaultGenerator,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ManifestInputDefaultGenerator {
    UuidV7,
    Ulid,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestEffects {
    pub(crate) version: u32,
    pub(crate) operations: Vec<ManifestEffect>,
    pub(crate) fallback: ManifestRevalidationFallback,
}

impl Serialize for ManifestEffects {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(Serialize)]
        struct Wire<'a> {
            version: u32,
            operations: &'a [JsonValue],
            fallback: ManifestRevalidationFallback,
        }

        let operations = self
            .operations
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .map_err(serde::ser::Error::custom)?;
        Wire {
            version: self.version,
            operations: &operations,
            fallback: self.fallback,
        }
        .serialize(serializer)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ManifestEffect {
    Upsert {
        model: String,
        key: ManifestEffectKey,
        fields: Vec<ManifestEffectField>,
    },
    Patch {
        model: String,
        key: ManifestEffectKey,
        fields: Vec<ManifestEffectField>,
    },
    Delete {
        model: String,
        key: ManifestEffectKey,
    },
    Link {
        relationship: ManifestEffectRelationship,
        source: ManifestEffectKey,
        target: ManifestEffectKey,
    },
    Unlink {
        relationship: ManifestEffectRelationship,
        source: ManifestEffectKey,
        target: ManifestEffectKey,
    },
    InvalidateModel {
        model: String,
    },
    InvalidateRelationship {
        relationship: ManifestEffectRelationship,
        source: ManifestEffectKey,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestEffectRelationship {
    pub(crate) source_model: String,
    pub(crate) field: String,
    pub(crate) target_model: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestEffectKey {
    pub(crate) fields: Vec<ManifestEffectField>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestEffectField {
    pub(crate) field: String,
    pub(crate) value: ManifestEffectExpression,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ManifestEffectExpression {
    Input { path: Vec<String> },
    TrustedPreset { name: String },
    Constant { value: JsonValue },
    Null,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ManifestRevalidationFallback {
    Revalidate,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestConfirmations {
    pub(crate) version: u32,
    pub(crate) kind: ManifestConfirmationKind,
    pub(crate) expected: Vec<ManifestConfirmation>,
    pub(crate) fallback: ManifestRevalidationFallback,
}

impl Serialize for ManifestConfirmations {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(Serialize)]
        struct Wire<'a> {
            version: u32,
            kind: ManifestConfirmationKind,
            expected: &'a [JsonValue],
            fallback: ManifestRevalidationFallback,
        }

        let expected = self
            .expected
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .map_err(serde::ser::Error::custom)?;
        Wire {
            version: self.version,
            kind: self.kind,
            expected: &expected,
            fallback: self.fallback,
        }
        .serialize(serializer)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ManifestConfirmationKind {
    Finite,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestConfirmation {
    pub(crate) projector: String,
    pub(crate) model: String,
    pub(crate) key: ManifestEffectKey,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) partition: Option<ManifestEffectExpression>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestProtocolOperations {
    pub(crate) version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) command_status: Option<ManifestProtocolOperation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestProtocolOperation {
    pub(crate) name: String,
    pub(crate) operation: String,
    pub(crate) operation_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestProjector {
    pub(crate) version: u32,
    pub(crate) name: String,
    pub(crate) facts: Vec<String>,
    pub(crate) models: Vec<String>,
    pub(crate) dependencies: Vec<String>,
    pub(crate) causal_confirmation: bool,
}

// The compiler treats one manifest version as an exact executable contract.
// Same-version extensions must bump the pre-release version instead of being
// silently ignored by an older client compiler.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestWire {
    manifest_version: u64,
    protocol_version: u64,
    service_id: String,
    surface: ManifestSurface,
    schema_fingerprint: String,
    protocol_fingerprint: String,
    execution: ManifestExecutionLimits,
    capabilities: ManifestCapabilities,
    scalar_codecs: Vec<ManifestScalarCodec>,
    models: Vec<ManifestModel>,
    roots: Vec<ManifestRoot>,
    commands: Vec<ManifestCommand>,
    protocol_operations: ManifestProtocolOperations,
    projectors: Vec<ManifestProjector>,
}

#[derive(Serialize)]
struct ManifestSchemaMaterial<'a> {
    manifest_version: u64,
    protocol_version: u64,
    service_id: &'a str,
    surface: &'a ManifestSurface,
    execution: &'a ManifestExecutionLimits,
    capabilities: &'a ManifestCapabilities,
    scalar_codecs: &'a [ManifestScalarCodec],
    models: &'a [ManifestModel],
    roots: &'a [ManifestRoot],
    commands: &'a [ManifestCommand],
    protocol_operations: &'a ManifestProtocolOperations,
    projectors: &'a [ManifestProjector],
}

impl ClientManifest {
    pub(crate) fn parse(
        value: JsonValue,
        selector: &ClientSurfaceSelector,
    ) -> Result<Self, ClientCompileError> {
        validate_input_default_generators_in_json(&value)?;
        let mut wire: ManifestWire = serde_json::from_value(value).map_err(|error| {
            ClientCompileError::manifest(
                "client.manifest.invalid",
                format!("invalid Distributed client manifest: {error}"),
            )
        })?;
        let computed_schema_fingerprint = schema_fingerprint(&wire)?;
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
        validate_execution_limits(&wire.execution)?;
        if wire.protocol_fingerprint != PROTOCOL_FINGERPRINT {
            return Err(ClientCompileError::manifest(
                "client.manifest.protocol_fingerprint",
                format!(
                    "client compiler protocol contract is `{PROTOCOL_FINGERPRINT}`, received `{}`; regenerate the manifest and use a matching dctl version",
                    wire.protocol_fingerprint
                ),
            ));
        }
        canonicalize_surface(&mut wire.surface)?;
        validate_surface(&wire.surface, selector)?;
        validate_capabilities(&wire.capabilities)?;

        let scalar_codecs = validate_scalar_codecs(wire.scalar_codecs)?;
        let mut models = BTreeMap::new();
        let mut typenames = BTreeSet::new();
        let mut source_tables = BTreeSet::new();
        let mut filter_input_types = BTreeSet::new();
        for mut model in wire.models {
            canonicalize_model(&mut model)?;
            validate_model(&model, &scalar_codecs)?;
            if !typenames.insert(model.typename.clone()) {
                return Err(ClientCompileError::manifest(
                    "client.manifest.duplicate_typename",
                    format!("duplicate manifest model typename `{}`", model.typename),
                ));
            }
            if !source_tables.insert(model.source_table.clone()) {
                return Err(ClientCompileError::manifest(
                    "client.manifest.duplicate_source_table",
                    format!(
                        "multiple manifest models claim source table `{}`",
                        model.source_table
                    ),
                ));
            }
            if !filter_input_types.insert(model.filter_input.type_name.clone()) {
                return Err(ClientCompileError::manifest(
                    "client.manifest.duplicate_filter_input",
                    format!(
                        "multiple manifest models claim filter input type `{}`",
                        model.filter_input.type_name
                    ),
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
        validate_model_graph(&models, &scalar_codecs)?;

        let mut roots = BTreeMap::new();
        let mut root_ids = BTreeSet::new();
        for mut root in wire.roots {
            canonicalize_root(&mut root)?;
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
            validate_unique_arguments(&root, &scalar_codecs)?;
            validate_root_contract(&root, &models)?;
            let key = (root.operation, root.name.clone());
            if roots.insert(key, root).is_some() {
                return Err(ClientCompileError::manifest(
                    "client.manifest.duplicate_root",
                    "duplicate manifest operation root",
                ));
            }
        }

        let mut projectors = wire.projectors;
        canonicalize_projectors(&mut projectors)?;
        validate_projectors(&projectors, &models)?;
        let mut commands = wire.commands;
        canonicalize_commands(&mut commands)?;
        let mut command_validation = super::command_manifest::validate_command_manifest(
            &commands,
            &models,
            &roots,
            &scalar_codecs,
            &projectors,
            wire.capabilities.causal_receipts,
            &wire.protocol_operations,
        )?;
        command_validation
            .commands_requiring_revalidation
            .extend(validate_direct_projections(
                &commands,
                &models,
                &projectors,
            )?);
        validate_derived_capabilities(&wire.capabilities, &roots, &commands)?;
        if wire.schema_fingerprint != computed_schema_fingerprint {
            return Err(ClientCompileError::manifest(
                "client.manifest.schema_fingerprint",
                format!(
                    "manifest schema fingerprint mismatch: declared `{}`, computed `{computed_schema_fingerprint}`; regenerate the selected client manifest",
                    wire.schema_fingerprint
                ),
            ));
        }

        Ok(Self {
            service_id: wire.service_id,
            surface: wire.surface,
            schema_fingerprint: wire.schema_fingerprint,
            protocol_fingerprint: wire.protocol_fingerprint,
            execution: wire.execution,
            capabilities: wire.capabilities,
            scalar_codecs,
            models,
            roots,
            commands,
            commands_requiring_revalidation: command_validation.commands_requiring_revalidation,
            protocol_operations: wire.protocol_operations,
            projectors,
        })
    }

    pub(crate) fn root(&self, operation: RootOperation, name: &str) -> Option<&ManifestRoot> {
        self.roots.get(&(operation, name.to_string()))
    }
}

fn schema_fingerprint(wire: &ManifestWire) -> Result<String, ClientCompileError> {
    let material = ManifestSchemaMaterial {
        manifest_version: wire.manifest_version,
        protocol_version: wire.protocol_version,
        service_id: &wire.service_id,
        surface: &wire.surface,
        execution: &wire.execution,
        capabilities: &wire.capabilities,
        scalar_codecs: &wire.scalar_codecs,
        models: &wire.models,
        roots: &wire.roots,
        commands: &wire.commands,
        protocol_operations: &wire.protocol_operations,
        projectors: &wire.projectors,
    };
    serde_json::to_vec(&material)
        .map(|bytes| hash_bytes(&bytes))
        .map_err(|error| {
            ClientCompileError::manifest(
                "client.manifest.schema_fingerprint",
                format!("could not recompute manifest schema fingerprint: {error}"),
            )
        })
}

#[cfg(test)]
pub(crate) fn refresh_schema_fingerprint(value: &mut JsonValue) {
    let wire: ManifestWire =
        serde_json::from_value(value.clone()).expect("test manifest must match the v5 wire shape");
    value["schema_fingerprint"] =
        JsonValue::String(schema_fingerprint(&wire).expect("test manifest must be serializable"));
}

fn validate_input_default_generators_in_json(value: &JsonValue) -> Result<(), ClientCompileError> {
    let Some(commands) = value.get("commands").and_then(JsonValue::as_array) else {
        return Ok(());
    };
    for (command_index, command) in commands.iter().enumerate() {
        let command_name = command
            .get("name")
            .and_then(JsonValue::as_str)
            .unwrap_or("<unknown>");
        let Some(defaults) = command
            .pointer("/extensions/input_defaults/defaults")
            .and_then(JsonValue::as_array)
        else {
            continue;
        };
        for (default_index, default) in defaults.iter().enumerate() {
            if !matches!(
                default.get("generator").and_then(JsonValue::as_str),
                Some("uuid_v7" | "ulid")
            ) {
                return Err(ClientCompileError::manifest(
                    "client.manifest.input_default_generator",
                    format!(
                        "manifest command `{command_name}` input default {default_index} must use uuid_v7 or ulid (commands[{command_index}])"
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn canonicalize_surface(surface: &mut ManifestSurface) -> Result<(), ClientCompileError> {
    match surface {
        ManifestSurface::Role { name } => {
            validate_nonempty(name, "manifest.surface.name")?;
        }
        ManifestSurface::Application { name, roles } => {
            validate_nonempty(name, "manifest.surface.name")?;
            if roles.is_empty() {
                return Err(ClientCompileError::manifest(
                    "client.manifest.surface_roles",
                    format!("application surface `{name}` must declare at least one role"),
                ));
            }
            canonicalize_string_set(roles, &format!("application surface `{name}` role"))?;
        }
    }
    Ok(())
}

fn canonicalize_string_set(values: &mut [String], label: &str) -> Result<(), ClientCompileError> {
    validate_nonempty_strings(values, label)?;
    values.sort();
    if values.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(ClientCompileError::manifest(
            "client.manifest.duplicate_entry",
            format!("{label} entries must be unique"),
        ));
    }
    Ok(())
}

fn validate_nonempty_strings(values: &[String], label: &str) -> Result<(), ClientCompileError> {
    for value in values {
        validate_nonempty(value, label)?;
    }
    Ok(())
}

fn require_dependency(
    dependencies: &[String],
    required: &str,
    owner: &str,
) -> Result<(), ClientCompileError> {
    if dependencies.iter().any(|dependency| dependency == required) {
        return Ok(());
    }
    Err(ClientCompileError::manifest(
        "client.manifest.dependency",
        format!("{owner} must include invalidation dependency `{required}`"),
    ))
}

fn validate_graphql_name(value: &str, label: &str) -> Result<(), ClientCompileError> {
    if !super::is_graphql_name(value) || value.starts_with("__") {
        return Err(ClientCompileError::manifest(
            "client.manifest.graphql_name",
            format!("{label} `{value}` must be a valid GraphQL name"),
        ));
    }
    Ok(())
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

fn validate_capabilities(capabilities: &ManifestCapabilities) -> Result<(), ClientCompileError> {
    if capabilities.query_fallback != "revalidate" {
        return Err(ClientCompileError::manifest(
            "client.manifest.query_fallback",
            format!(
                "unsupported query fallback `{}`; manifest v7 requires `revalidate`",
                capabilities.query_fallback
            ),
        ));
    }
    if capabilities.live_resume && !capabilities.live_queries {
        return Err(ClientCompileError::manifest(
            "client.manifest.live_resume",
            "capabilities.live_resume requires capabilities.live_queries",
        ));
    }
    if capabilities.tombstones && !capabilities.record_revisions {
        return Err(ClientCompileError::manifest(
            "client.manifest.tombstone_capability",
            "capabilities.tombstones requires capabilities.record_revisions",
        ));
    }
    Ok(())
}

fn validate_execution_limits(
    execution: &ManifestExecutionLimits,
) -> Result<(), ClientCompileError> {
    const JS_MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
    for (name, value) in [
        ("max_depth", execution.max_depth),
        ("max_bool_width", execution.max_bool_width),
        ("max_in_list", execution.max_in_list),
    ] {
        if value > JS_MAX_SAFE_INTEGER {
            return Err(ClientCompileError::manifest(
                "client.manifest.execution_js_integer",
                format!("execution.{name} `{value}` exceeds JavaScript's exact integer range"),
            ));
        }
    }
    if execution.complexity.version != 1 {
        return Err(ClientCompileError::manifest(
            "client.manifest.complexity_version",
            format!(
                "unsupported query complexity contract version {}; dctl requires version 1",
                execution.complexity.version
            ),
        ));
    }
    let weights = [
        ("scalar", execution.complexity.scalar),
        ("belongs_to", execution.complexity.belongs_to),
        ("has_many", execution.complexity.has_many),
        ("m2m", execution.complexity.m2m),
        ("aggregate", execution.complexity.aggregate),
        ("list_root", execution.complexity.list_root),
        ("by_pk", execution.complexity.by_pk),
        ("list_fanout", execution.complexity.list_fanout),
    ];
    if let Some((name, _)) = weights.into_iter().find(|(_, value)| *value == 0) {
        return Err(ClientCompileError::manifest(
            "client.manifest.complexity_weight",
            format!("query complexity weight `{name}` must be greater than zero"),
        ));
    }
    Ok(())
}

fn validate_derived_capabilities(
    capabilities: &ManifestCapabilities,
    roots: &BTreeMap<(RootOperation, String), ManifestRoot>,
    commands: &[ManifestCommand],
) -> Result<(), ClientCompileError> {
    let has_live_roots = roots
        .keys()
        .any(|(operation, _)| *operation == RootOperation::Subscription);
    if capabilities.live_queries != has_live_roots {
        return Err(ClientCompileError::manifest(
            "client.manifest.live_capability",
            "capabilities.live_queries must exactly describe the subscription-root inventory",
        ));
    }
    let has_commands = !commands.is_empty();
    if capabilities.causal_receipts != has_commands || !capabilities.cache_scope {
        return Err(ClientCompileError::manifest(
            "client.manifest.command_capability",
            "causal_receipts must agree with command inventory and cache_scope must be enabled for every generated surface",
        ));
    }
    if capabilities.confirmed_persistence {
        return Err(ClientCompileError::manifest(
            "client.manifest.persistence_capability",
            "manifest v7 does not yet support confirmed client persistence",
        ));
    }
    Ok(())
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
    if result.len() != supported.len()
        || supported.keys().any(|scalar| !result.contains_key(*scalar))
    {
        return Err(ClientCompileError::manifest(
            "client.manifest.scalar_codecs",
            "manifest.scalar_codecs must contain the exact v4 scalar inventory",
        ));
    }
    Ok(result)
}

fn canonicalize_model(model: &mut ManifestModel) -> Result<(), ClientCompileError> {
    canonicalize_string_set(
        &mut model.dependencies,
        &format!("model `{}` dependency", model.id),
    )?;
    model
        .fields
        .sort_by(|left, right| left.name.cmp(&right.name));
    model
        .relationships
        .sort_by(|left, right| left.name.cmp(&right.name));
    canonicalize_filter_input(&mut model.filter_input)?;
    canonicalize_row_policy(&mut model.row_policy);
    for relationship in &mut model.relationships {
        canonicalize_arguments(
            &mut relationship.arguments,
            &format!(
                "model `{}` relationship `{}` argument",
                model.id, relationship.name
            ),
        )?;
        canonicalize_string_set(
            &mut relationship.dependencies,
            &format!(
                "model `{}` relationship `{}` dependency",
                model.id, relationship.name
            ),
        )?;
        if let Some(filter) = &mut relationship.filter {
            canonicalize_filter_semantics(filter)?;
        }
        if let Some(order) = &mut relationship.order {
            canonicalize_order_semantics(order)?;
        }
        if let Some(aggregate) = &mut relationship.aggregate {
            canonicalize_arguments(
                &mut aggregate.arguments,
                &format!(
                    "model `{}` relationship `{}` aggregate argument",
                    model.id, relationship.name
                ),
            )?;
            canonicalize_aggregate_semantics(&mut aggregate.semantics)?;
            canonicalize_string_set(
                &mut aggregate.dependencies,
                &format!(
                    "model `{}` relationship `{}` aggregate dependency",
                    model.id, relationship.name
                ),
            )?;
        }
    }
    Ok(())
}

fn canonicalize_row_policy(policy: &mut ManifestRowPolicy) {
    if let ManifestRowPolicy::Predicate { expression } = policy {
        canonicalize_filter_expression(expression);
    }
}

fn canonicalize_filter_expression(expression: &mut ManifestFilterExpr) {
    match expression {
        ManifestFilterExpr::And(expressions) | ManifestFilterExpr::Or(expressions) => {
            for expression in expressions {
                canonicalize_filter_expression(expression);
            }
        }
        ManifestFilterExpr::Not(expression) => canonicalize_filter_expression(expression),
        ManifestFilterExpr::Cmp { rhs, .. } => canonicalize_operand(rhs),
        ManifestFilterExpr::In { values, .. } => {
            for operand in values {
                canonicalize_operand(operand);
            }
        }
        ManifestFilterExpr::Rel { predicate, .. } => {
            canonicalize_filter_expression(predicate);
        }
        ManifestFilterExpr::IsNull { .. } => {}
    }
}

fn canonicalize_operand(operand: &mut ManifestOperand) {
    if let ManifestOperand::Lit(ManifestLitValue::Json(value)) = operand {
        *value = canonical_json_value(std::mem::take(value));
    }
}

fn canonicalize_filter_semantics(
    semantics: &mut ManifestFilterSemantics,
) -> Result<(), ClientCompileError> {
    canonicalize_filter_fields(&mut semantics.fields)?;
    canonicalize_string_set(&mut semantics.relationships, "filter relationship")?;
    canonicalize_row_policy(&mut semantics.row_policy);
    Ok(())
}

fn canonicalize_filter_input(input: &mut ManifestFilterInput) -> Result<(), ClientCompileError> {
    canonicalize_filter_fields(&mut input.fields)?;
    input
        .relationships
        .sort_by(|left, right| left.field.cmp(&right.field));
    Ok(())
}

fn canonicalize_filter_fields(
    fields: &mut [ManifestFilterField],
) -> Result<(), ClientCompileError> {
    fields.sort_by(|left, right| left.name.cmp(&right.name));
    for field in fields {
        canonicalize_string_set(
            &mut field.operators,
            &format!("filter field `{}` operator", field.name),
        )?;
    }
    Ok(())
}

fn canonicalize_order_semantics(
    semantics: &mut ManifestOrderSemantics,
) -> Result<(), ClientCompileError> {
    canonicalize_string_set(&mut semantics.fields, "order field")?;
    canonicalize_string_set(&mut semantics.values, "order value")
}

fn canonicalize_aggregate_semantics(
    semantics: &mut ManifestAggregateSemantics,
) -> Result<(), ClientCompileError> {
    canonicalize_string_set(&mut semantics.sum, "aggregate sum field")?;
    canonicalize_string_set(&mut semantics.avg, "aggregate avg field")?;
    canonicalize_string_set(&mut semantics.min, "aggregate min field")?;
    canonicalize_string_set(&mut semantics.max, "aggregate max field")
}

fn canonicalize_arguments(
    arguments: &mut [ManifestArgument],
    _label: &str,
) -> Result<(), ClientCompileError> {
    arguments.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(())
}

fn validate_model(
    model: &ManifestModel,
    scalar_codecs: &BTreeMap<String, String>,
) -> Result<(), ClientCompileError> {
    validate_graphql_name(&model.id, "manifest model id")?;
    validate_graphql_name(&model.typename, "manifest model typename")?;
    validate_nonempty(&model.source_table, "manifest model source_table")?;
    validate_graphql_name(
        &model.filter_input.type_name,
        "manifest model filter input type",
    )?;
    validate_nonempty_strings(
        &model.dependencies,
        &format!("model `{}` dependency", model.id),
    )?;
    if !model
        .dependencies
        .iter()
        .any(|dependency| dependency == &model.source_table)
    {
        return Err(ClientCompileError::manifest(
            "client.manifest.model_dependency",
            format!(
                "model `{}` dependencies must include source table `{}`",
                model.id, model.source_table
            ),
        ));
    }
    if model.tombstones && !model.record_revisions {
        return Err(ClientCompileError::manifest(
            "client.manifest.model_tombstones",
            format!(
                "model `{}` cannot expose tombstones without record revisions",
                model.id
            ),
        ));
    }
    let mut names = BTreeSet::new();
    for field in &model.fields {
        validate_graphql_name(&field.name, "manifest model field")?;
        validate_graphql_name(&field.scalar, "manifest field scalar")?;
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
        validate_graphql_name(&relationship.name, "manifest relationship")?;
        if !names.insert(relationship.name.as_str()) {
            return Err(ClientCompileError::manifest(
                "client.manifest.duplicate_member",
                format!(
                    "model `{}` repeats field/relationship `{}`",
                    model.id, relationship.name
                ),
            ));
        }
        validate_graphql_name(
            &relationship.target_model,
            "manifest relationship target model",
        )?;
        validate_graphql_name(
            &relationship.target_typename,
            "manifest relationship target typename",
        )?;
        validate_unique_arguments_for(
            &relationship.arguments,
            scalar_codecs,
            &format!("model `{}` relationship `{}`", model.id, relationship.name),
        )?;
        validate_nonempty_strings(
            &relationship.dependencies,
            &format!(
                "model `{}` relationship `{}` dependency",
                model.id, relationship.name
            ),
        )?;
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
                validate_graphql_name(&identity.name, "manifest identity field")?;
                validate_nonempty(&identity.codec, "manifest identity codec")?;
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

fn validate_model_graph(
    models: &BTreeMap<String, ManifestModel>,
    scalar_codecs: &BTreeMap<String, String>,
) -> Result<(), ClientCompileError> {
    for model in models.values() {
        validate_row_policy(&model.row_policy, model, models)?;
        validate_filter_input(&model.filter_input, model, models)?;
        for relationship in &model.relationships {
            let target = models.get(&relationship.target_model).ok_or_else(|| {
                ClientCompileError::manifest(
                    "client.manifest.relationship_target",
                    format!(
                        "model `{}` relationship `{}` targets absent model `{}`",
                        model.id, relationship.name, relationship.target_model
                    ),
                )
            })?;
            if relationship.target_typename != target.typename {
                return Err(ClientCompileError::manifest(
                    "client.manifest.relationship_typename",
                    format!(
                        "model `{}` relationship `{}` target typename `{}` does not match model `{}` typename `{}`",
                        model.id,
                        relationship.name,
                        relationship.target_typename,
                        target.id,
                        target.typename
                    ),
                ));
            }
            require_dependency(
                &relationship.dependencies,
                &model.source_table,
                &format!("relationship {}.{}", model.id, relationship.name),
            )?;
            require_dependency(
                &relationship.dependencies,
                &target.source_table,
                &format!("relationship {}.{}", model.id, relationship.name),
            )?;
            if let ManifestRelationshipKeyMapping::Through { table, .. } = &relationship.key_mapping
            {
                require_dependency(
                    &relationship.dependencies,
                    table,
                    &format!("relationship {}.{}", model.id, relationship.name),
                )?;
            }
            match relationship.kind {
                ManifestRelationshipKind::BelongsTo if relationship.list => {
                    return Err(ClientCompileError::manifest(
                        "client.manifest.relationship_cardinality",
                        format!(
                            "belongs_to relationship `{}.{}` cannot be a list",
                            model.id, relationship.name
                        ),
                    ));
                }
                ManifestRelationshipKind::HasMany | ManifestRelationshipKind::ManyToMany
                    if !relationship.list =>
                {
                    return Err(ClientCompileError::manifest(
                        "client.manifest.relationship_cardinality",
                        format!(
                            "{:?} relationship `{}.{}` must be a list",
                            relationship.kind, model.id, relationship.name
                        ),
                    ));
                }
                _ => {}
            }
            if relationship.list && relationship.nullable {
                return Err(ClientCompileError::manifest(
                    "client.manifest.relationship_nullability",
                    format!(
                        "list relationship `{}.{}` must be a non-null collection",
                        model.id, relationship.name
                    ),
                ));
            }
            validate_key_mapping(model, target, relationship)?;
            validate_relationship_semantics(model, target, relationship, models, scalar_codecs)?;
        }
    }
    Ok(())
}

fn validate_key_mapping(
    source: &ManifestModel,
    target: &ManifestModel,
    relationship: &ManifestRelationship,
) -> Result<(), ClientCompileError> {
    let validate_fields = |local: &[String], remote: &[String]| {
        if local.is_empty() || local.len() != remote.len() {
            return Err(ClientCompileError::manifest(
                "client.manifest.relationship_key_mapping",
                format!(
                    "relationship `{}.{}` key mapping must contain equally sized non-empty local and remote fields",
                    source.id, relationship.name
                ),
            ));
        }
        for field in local {
            if source.field(field).is_none() {
                return Err(ClientCompileError::manifest(
                    "client.manifest.relationship_local_key",
                    format!(
                        "relationship `{}.{}` references absent local field `{field}`",
                        source.id, relationship.name
                    ),
                ));
            }
        }
        for field in remote {
            if target.field(field).is_none() {
                return Err(ClientCompileError::manifest(
                    "client.manifest.relationship_remote_key",
                    format!(
                        "relationship `{}.{}` references absent target field `{field}`",
                        source.id, relationship.name
                    ),
                ));
            }
        }
        for (local_field, remote_field) in local.iter().zip(remote) {
            let local_contract = source
                .field(local_field)
                .expect("local relationship field checked above");
            let remote_contract = target
                .field(remote_field)
                .expect("remote relationship field checked above");
            if local_contract.scalar == "BigInt"
                || remote_contract.scalar == "BigInt"
                || local_contract.codec != remote_contract.codec
            {
                return Err(ClientCompileError::manifest(
                    "client.manifest.relationship_key_codec",
                    format!(
                        "relationship {}.{} local and target keys must use matching portable codecs",
                        source.id, relationship.name
                    ),
                ));
            }
        }
        Ok(())
    };
    let local_maintenance = matches!(
        relationship.key_mapping,
        ManifestRelationshipKeyMapping::Direct { .. }
            | ManifestRelationshipKeyMapping::Through { .. }
    );
    if local_maintenance
        != matches!(
            relationship.maintenance,
            ManifestRelationshipMaintenance::Local
        )
    {
        return Err(ClientCompileError::manifest(
            "client.manifest.relationship_maintenance",
            format!(
                "relationship `{}.{}` maintenance does not match its key mapping",
                source.id, relationship.name
            ),
        ));
    }
    match &relationship.key_mapping {
        ManifestRelationshipKeyMapping::Direct { local, remote } => validate_fields(local, remote),
        ManifestRelationshipKeyMapping::Through {
            local,
            remote,
            table,
            source_foreign_key,
            target_foreign_key,
        } => {
            validate_fields(local, remote)?;
            validate_nonempty(table, "relationship through table")?;
            validate_nonempty(
                source_foreign_key,
                "relationship through source foreign key",
            )?;
            validate_nonempty(
                target_foreign_key,
                "relationship through target foreign key",
            )
        }
        ManifestRelationshipKeyMapping::ThroughOpaque {
            local,
            remote,
            dependency,
        } => {
            validate_fields(local, remote)?;
            validate_nonempty(dependency, "opaque relationship dependency")?;
            if !relationship
                .dependencies
                .iter()
                .any(|candidate| candidate == dependency)
            {
                return Err(ClientCompileError::manifest(
                    "client.manifest.relationship_dependency",
                    format!(
                        "relationship `{}.{}` opaque dependency `{dependency}` is absent from its dependency set",
                        source.id, relationship.name
                    ),
                ));
            }
            Ok(())
        }
        ManifestRelationshipKeyMapping::Embedded => Ok(()),
    }
}

fn validate_relationship_semantics(
    source: &ManifestModel,
    target: &ManifestModel,
    relationship: &ManifestRelationship,
    models: &BTreeMap<String, ManifestModel>,
    scalar_codecs: &BTreeMap<String, String>,
) -> Result<(), ClientCompileError> {
    validate_filter_argument_type(
        &relationship.arguments,
        target,
        &format!("relationship `{}.{}`", source.id, relationship.name),
    )?;
    let has_filter_argument = relationship
        .arguments
        .iter()
        .any(|argument| argument.kind == ManifestArgumentKind::Filter);
    let has_order_argument = relationship
        .arguments
        .iter()
        .any(|argument| argument.kind == ManifestArgumentKind::Order);
    let has_limit_argument = relationship
        .arguments
        .iter()
        .any(|argument| argument.kind == ManifestArgumentKind::Limit);
    let has_offset_argument = relationship
        .arguments
        .iter()
        .any(|argument| argument.kind == ManifestArgumentKind::Offset);
    if relationship.filter.is_some() != has_filter_argument
        || relationship.order.is_some() != has_order_argument
        || relationship.pagination.is_some() != (has_limit_argument && has_offset_argument)
    {
        return Err(ClientCompileError::manifest(
            "client.manifest.relationship_arguments",
            format!(
                "relationship {}.{} arguments do not match its filter/order/pagination semantics",
                source.id, relationship.name
            ),
        ));
    }
    if relationship.list {
        let filter = relationship.filter.as_ref().ok_or_else(|| {
            ClientCompileError::manifest(
                "client.manifest.relationship_filter",
                format!(
                    "list relationship `{}.{}` requires filter semantics",
                    source.id, relationship.name
                ),
            )
        })?;
        validate_filter_semantics(filter, target, models)?;
        let order = relationship.order.as_ref().ok_or_else(|| {
            ClientCompileError::manifest(
                "client.manifest.relationship_order",
                format!(
                    "list relationship `{}.{}` requires order semantics",
                    source.id, relationship.name
                ),
            )
        })?;
        validate_order_semantics(order, target)?;
        validate_pagination(
            relationship.pagination.as_ref().ok_or_else(|| {
                ClientCompileError::manifest(
                    "client.manifest.relationship_pagination",
                    format!(
                        "list relationship `{}.{}` requires pagination semantics",
                        source.id, relationship.name
                    ),
                )
            })?,
            &format!("relationship `{}.{}`", source.id, relationship.name),
        )?;
    } else if relationship.filter.is_some()
        || relationship.order.is_some()
        || relationship.pagination.is_some()
    {
        return Err(ClientCompileError::manifest(
            "client.manifest.relationship_list_semantics",
            format!(
                "singular relationship `{}.{}` cannot declare filter, order, or pagination semantics",
                source.id, relationship.name
            ),
        ));
    }
    if relationship.live && !relationship.list {
        return Err(ClientCompileError::manifest(
            "client.manifest.relationship_live",
            format!(
                "singular relationship `{}.{}` cannot be marked live",
                source.id, relationship.name
            ),
        ));
    }
    if let Some(aggregate) = &relationship.aggregate {
        if !relationship.list {
            return Err(ClientCompileError::manifest(
                "client.manifest.relationship_aggregate",
                format!(
                    "singular relationship `{}.{}` cannot expose an aggregate",
                    source.id, relationship.name
                ),
            ));
        }
        validate_graphql_name(&aggregate.name, "relationship aggregate name")?;
        validate_graphql_name(
            &aggregate.semantics.wrapper_typename,
            "relationship aggregate wrapper type",
        )?;
        validate_graphql_name(
            &aggregate.semantics.fields_typename,
            "relationship aggregate fields type",
        )?;
        validate_unique_arguments_for(
            &aggregate.arguments,
            scalar_codecs,
            &format!("relationship aggregate {}.{}", source.id, aggregate.name),
        )?;
        validate_nonempty_strings(&aggregate.dependencies, "relationship aggregate dependency")?;
        for dependency in &relationship.dependencies {
            require_dependency(
                &aggregate.dependencies,
                dependency,
                &format!("relationship aggregate {}.{}", source.id, aggregate.name),
            )?;
        }
        validate_aggregate_semantics(&aggregate.semantics, target)?;
    }
    Ok(())
}

fn validate_filter_input(
    input: &ManifestFilterInput,
    model: &ManifestModel,
    models: &BTreeMap<String, ManifestModel>,
) -> Result<(), ClientCompileError> {
    validate_filter_fields(&input.fields, model)?;
    let expected_relationships = model
        .relationships
        .iter()
        .map(|relationship| relationship.name.as_str())
        .collect::<BTreeSet<_>>();
    let actual_relationships = input
        .relationships
        .iter()
        .map(|relationship| relationship.field.as_str())
        .collect::<BTreeSet<_>>();
    if actual_relationships != expected_relationships
        || input.relationships.len() != expected_relationships.len()
    {
        return Err(ClientCompileError::manifest(
            "client.manifest.filter_input_relationships",
            format!(
                "model `{}` filter input must describe every authorized relationship exactly once",
                model.id
            ),
        ));
    }
    for relationship_input in &input.relationships {
        validate_graphql_name(
            &relationship_input.field,
            "manifest filter input relationship field",
        )?;
        validate_graphql_name(
            &relationship_input.target_type,
            "manifest filter input relationship target type",
        )?;
        let relationship = model
            .relationship(&relationship_input.field)
            .expect("filter input relationship inventory checked above");
        let target = models.get(&relationship.target_model).ok_or_else(|| {
            ClientCompileError::manifest(
                "client.manifest.filter_input_target",
                format!(
                    "model `{}` filter relationship `{}` targets absent model `{}`",
                    model.id, relationship.name, relationship.target_model
                ),
            )
        })?;
        if relationship_input.target_type != target.filter_input.type_name {
            return Err(ClientCompileError::manifest(
                "client.manifest.filter_input_target_type",
                format!(
                    "model `{}` filter relationship `{}` target type `{}` does not match model `{}` filter input `{}`",
                    model.id,
                    relationship.name,
                    relationship_input.target_type,
                    target.id,
                    target.filter_input.type_name
                ),
            ));
        }
    }
    Ok(())
}

fn validate_filter_fields(
    fields: &[ManifestFilterField],
    model: &ManifestModel,
) -> Result<(), ClientCompileError> {
    let expected_fields = model
        .fields
        .iter()
        .map(|field| field.name.as_str())
        .collect::<BTreeSet<_>>();
    let actual_fields = fields
        .iter()
        .map(|field| field.name.as_str())
        .collect::<BTreeSet<_>>();
    if actual_fields != expected_fields || fields.len() != expected_fields.len() {
        return Err(ClientCompileError::manifest(
            "client.manifest.filter_fields",
            format!(
                "filter input for model `{}` must describe every authorized scalar field exactly once",
                model.id
            ),
        ));
    }
    for field in fields {
        validate_nonempty_strings(
            &field.operators,
            &format!("model `{}` filter operator", model.id),
        )?;
        if field.operators.is_empty() {
            return Err(ClientCompileError::manifest(
                "client.manifest.filter_operators",
                format!(
                    "model `{}` filter field `{}` has no supported operators",
                    model.id, field.name
                ),
            ));
        }
        if field.operators.iter().any(|operator| {
            !matches!(
                operator.as_str(),
                "_eq"
                    | "_neq"
                    | "_gt"
                    | "_gte"
                    | "_lt"
                    | "_lte"
                    | "_in"
                    | "_nin"
                    | "_is_null"
                    | "_like"
                    | "_ilike"
                    | "_contains"
                    | "_contained_in"
                    | "_has_key"
            )
        }) {
            return Err(ClientCompileError::manifest(
                "client.manifest.filter_operator",
                format!(
                    "model `{}` filter field `{}` declares an unknown comparison operator",
                    model.id, field.name
                ),
            ));
        }
    }
    Ok(())
}

fn validate_filter_semantics(
    semantics: &ManifestFilterSemantics,
    model: &ManifestModel,
    models: &BTreeMap<String, ManifestModel>,
) -> Result<(), ClientCompileError> {
    if semantics.fields != model.filter_input.fields {
        return Err(ClientCompileError::manifest(
            "client.manifest.filter_contract",
            format!(
                "filter semantics for model `{}` do not match its authoritative filter input fields",
                model.id
            ),
        ));
    }
    let input_relationships = model
        .filter_input
        .relationships
        .iter()
        .map(|relationship| relationship.field.as_str())
        .collect::<Vec<_>>();
    if semantics
        .relationships
        .iter()
        .map(String::as_str)
        .ne(input_relationships)
    {
        return Err(ClientCompileError::manifest(
            "client.manifest.filter_contract",
            format!(
                "filter semantics for model `{}` do not match its authoritative filter input relationships",
                model.id
            ),
        ));
    }
    if semantics.row_policy != model.row_policy {
        return Err(ClientCompileError::manifest(
            "client.manifest.filter_row_policy",
            format!(
                "filter semantics for model `{}` do not preserve its row policy",
                model.id
            ),
        ));
    }
    validate_row_policy(&semantics.row_policy, model, models)
}

fn validate_filter_argument_type(
    arguments: &[ManifestArgument],
    model: &ManifestModel,
    owner: &str,
) -> Result<(), ClientCompileError> {
    let Some(argument) = arguments
        .iter()
        .find(|argument| argument.kind == ManifestArgumentKind::Filter)
    else {
        return Ok(());
    };
    if argument.list || argument.type_name != model.filter_input.type_name {
        return Err(ClientCompileError::manifest(
            "client.manifest.filter_argument_type",
            format!(
                "{owner} filter argument `{}` must use non-list input `{}`, received `{}`",
                argument.name, model.filter_input.type_name, argument.type_name
            ),
        ));
    }
    Ok(())
}

fn validate_order_semantics(
    semantics: &ManifestOrderSemantics,
    model: &ManifestModel,
) -> Result<(), ClientCompileError> {
    let expected_fields = model
        .fields
        .iter()
        .map(|field| field.name.as_str())
        .collect::<BTreeSet<_>>();
    let actual_fields = semantics
        .fields
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if actual_fields != expected_fields || semantics.fields.len() != expected_fields.len() {
        return Err(ClientCompileError::manifest(
            "client.manifest.order_fields",
            format!(
                "order semantics for model `{}` must describe every authorized scalar field exactly once",
                model.id
            ),
        ));
    }
    let expected_values = [
        "asc",
        "asc_nulls_first",
        "asc_nulls_last",
        "desc",
        "desc_nulls_first",
        "desc_nulls_last",
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    let actual_values = semantics
        .values
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if actual_values != expected_values || semantics.values.len() != expected_values.len() {
        return Err(ClientCompileError::manifest(
            "client.manifest.order_values",
            format!(
                "order semantics for model `{}` must use the manifest v7 direction set",
                model.id
            ),
        ));
    }
    Ok(())
}

fn validate_pagination(
    pagination: &ManifestPagination,
    owner: &str,
) -> Result<(), ClientCompileError> {
    if pagination.kind != "offset" || pagination.coverage != "window" {
        return Err(ClientCompileError::manifest(
            "client.manifest.pagination",
            format!("{owner} pagination must use kind `offset` and coverage `window`"),
        ));
    }
    if pagination.default_limit > pagination.max_limit {
        return Err(ClientCompileError::manifest(
            "client.manifest.pagination_limit",
            format!("{owner} pagination default_limit must not exceed max_limit"),
        ));
    }
    Ok(())
}

fn validate_aggregate_semantics(
    aggregate: &ManifestAggregateSemantics,
    model: &ManifestModel,
) -> Result<(), ClientCompileError> {
    validate_graphql_name(&aggregate.wrapper_typename, "aggregate wrapper typename")?;
    validate_graphql_name(&aggregate.fields_typename, "aggregate fields typename")?;
    validate_pagination(
        &aggregate.nodes_pagination,
        &format!("aggregate nodes for model `{}`", model.id),
    )?;
    if !aggregate.count || !aggregate.nodes {
        return Err(ClientCompileError::manifest(
            "client.manifest.aggregate_capability",
            format!(
                "aggregate semantics for model `{}` must retain count and nodes",
                model.id
            ),
        ));
    }
    for (label, fields) in [
        ("sum", &aggregate.sum),
        ("avg", &aggregate.avg),
        ("min", &aggregate.min),
        ("max", &aggregate.max),
    ] {
        for field in fields {
            if model.field(field).is_none() {
                return Err(ClientCompileError::manifest(
                    "client.manifest.aggregate_field",
                    format!(
                        "aggregate {label} for model `{}` references absent field `{field}`",
                        model.id
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn validate_row_policy(
    policy: &ManifestRowPolicy,
    model: &ManifestModel,
    models: &BTreeMap<String, ManifestModel>,
) -> Result<(), ClientCompileError> {
    match policy {
        ManifestRowPolicy::Unrestricted | ManifestRowPolicy::ServerOnly => Ok(()),
        ManifestRowPolicy::Predicate { expression } => {
            validate_filter_expression(expression, model, models)
        }
    }
}

fn validate_filter_expression(
    expression: &ManifestFilterExpr,
    model: &ManifestModel,
    models: &BTreeMap<String, ManifestModel>,
) -> Result<(), ClientCompileError> {
    match expression {
        ManifestFilterExpr::And(expressions) | ManifestFilterExpr::Or(expressions) => {
            for expression in expressions {
                validate_filter_expression(expression, model, models)?;
            }
            Ok(())
        }
        ManifestFilterExpr::Not(expression) => {
            validate_filter_expression(expression, model, models)
        }
        ManifestFilterExpr::Cmp { column, rhs, .. } => {
            validate_policy_column(model, column)?;
            validate_operand(rhs)
        }
        ManifestFilterExpr::In { column, values, .. } => {
            validate_policy_column(model, column)?;
            for operand in values {
                validate_operand(operand)?;
            }
            Ok(())
        }
        ManifestFilterExpr::IsNull { column, .. } => validate_policy_column(model, column),
        ManifestFilterExpr::Rel { field, predicate } => {
            let relationship = model.relationship(field).ok_or_else(|| {
                ClientCompileError::manifest(
                    "client.manifest.row_policy_relationship",
                    format!(
                        "model `{}` row policy references absent relationship `{field}`",
                        model.id
                    ),
                )
            })?;
            let target = models.get(&relationship.target_model).ok_or_else(|| {
                ClientCompileError::manifest(
                    "client.manifest.row_policy_relationship",
                    format!(
                        "model `{}` row policy relationship `{field}` has an absent target",
                        model.id
                    ),
                )
            })?;
            validate_filter_expression(predicate, target, models)
        }
    }
}

fn validate_policy_column(model: &ManifestModel, column: &str) -> Result<(), ClientCompileError> {
    if model.field(column).is_none() {
        return Err(ClientCompileError::manifest(
            "client.manifest.row_policy_column",
            format!(
                "model `{}` row policy references absent field `{column}`",
                model.id
            ),
        ));
    }
    Ok(())
}

fn validate_operand(operand: &ManifestOperand) -> Result<(), ClientCompileError> {
    const JS_MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

    fn json_is_portable(value: &JsonValue) -> bool {
        match value {
            JsonValue::Null | JsonValue::Bool(_) | JsonValue::String(_) => true,
            JsonValue::Number(number) => {
                if let Some(value) = number.as_i64() {
                    value.unsigned_abs() <= JS_MAX_SAFE_INTEGER
                } else if let Some(value) = number.as_u64() {
                    value <= JS_MAX_SAFE_INTEGER
                } else {
                    number.as_f64().is_some_and(f64::is_finite)
                }
            }
            JsonValue::Array(values) => values.iter().all(json_is_portable),
            JsonValue::Object(values) => values.values().all(json_is_portable),
        }
    }

    if let ManifestOperand::Claim(claim) = operand {
        validate_nonempty(&claim.header, "row policy claim header")?;
        return Err(ClientCompileError::manifest(
            "client.manifest.row_policy_portability",
            "client-visible row policies cannot contain claim-dependent predicates",
        ));
    }
    let portable = match operand {
        ManifestOperand::Claim(_) => unreachable!("claim returned above"),
        ManifestOperand::Lit(ManifestLitValue::I64(value)) => {
            value.unsigned_abs() <= JS_MAX_SAFE_INTEGER
        }
        ManifestOperand::Lit(ManifestLitValue::Json(value)) => json_is_portable(value),
        ManifestOperand::Lit(_) => true,
    };
    if !portable {
        return Err(ClientCompileError::manifest(
            "client.manifest.row_policy_portability",
            "client-visible row policies cannot contain JavaScript-unsafe numbers",
        ));
    }
    Ok(())
}

fn canonicalize_root(root: &mut ManifestRoot) -> Result<(), ClientCompileError> {
    canonicalize_arguments(
        &mut root.arguments,
        &format!("manifest root `{}` argument", root.name),
    )?;
    canonicalize_string_set(
        &mut root.dependencies,
        &format!("manifest root `{}` dependency", root.name),
    )?;
    if let Some(filter) = &mut root.filter {
        canonicalize_filter_semantics(filter)?;
    }
    if let Some(order) = &mut root.order {
        canonicalize_order_semantics(order)?;
    }
    if let Some(aggregate) = &mut root.aggregate {
        canonicalize_aggregate_semantics(aggregate)?;
    }
    Ok(())
}

fn validate_root_contract(
    root: &ManifestRoot,
    models: &BTreeMap<String, ManifestModel>,
) -> Result<(), ClientCompileError> {
    let expected_id = format!(
        "{}:{}",
        match root.operation {
            RootOperation::Query => "query",
            RootOperation::Subscription => "subscription",
        },
        root.name
    );
    if root.id != expected_id {
        return Err(ClientCompileError::manifest(
            "client.manifest.root_id",
            format!(
                "root `{}` id must be `{expected_id}`, received `{}`",
                root.name, root.id
            ),
        ));
    }
    validate_graphql_name(&root.name, "manifest root name")?;
    let model = models.get(&root.model).ok_or_else(|| {
        ClientCompileError::manifest(
            "client.manifest.root_model",
            format!(
                "manifest root `{}` references missing model `{}`",
                root.name, root.model
            ),
        )
    })?;
    validate_filter_argument_type(
        &root.arguments,
        model,
        &format!("manifest root `{}`", root.name),
    )?;
    validate_nonempty_strings(
        &root.dependencies,
        &format!("manifest root `{}` dependency", root.name),
    )?;
    require_dependency(
        &root.dependencies,
        &model.source_table,
        &format!("manifest root {}", root.name),
    )?;
    if root.operation == RootOperation::Subscription && root.kind != RootKind::List {
        return Err(ClientCompileError::manifest(
            "client.manifest.subscription_kind",
            format!(
                "subscription root `{}` must use list cardinality in manifest v7",
                root.name
            ),
        ));
    }
    if root.operation == RootOperation::Subscription && !root.live {
        return Err(ClientCompileError::manifest(
            "client.manifest.subscription_live",
            format!("subscription root `{}` must be marked live", root.name),
        ));
    }

    let has_filter_argument = root
        .arguments
        .iter()
        .any(|argument| argument.kind == ManifestArgumentKind::Filter);
    let has_order_argument = root
        .arguments
        .iter()
        .any(|argument| argument.kind == ManifestArgumentKind::Order);
    let has_limit_argument = root
        .arguments
        .iter()
        .any(|argument| argument.kind == ManifestArgumentKind::Limit);
    let has_offset_argument = root
        .arguments
        .iter()
        .any(|argument| argument.kind == ManifestArgumentKind::Offset);
    if root.filter.is_some() != has_filter_argument
        || root.order.is_some() != has_order_argument
        || root.pagination.is_some() != (has_limit_argument && has_offset_argument)
    {
        return Err(ClientCompileError::manifest(
            "client.manifest.root_arguments",
            format!(
                "root `{}` arguments do not match its filter/order/pagination semantics",
                root.name
            ),
        ));
    }
    if let Some(filter) = &root.filter {
        validate_filter_semantics(filter, model, models)?;
    }
    if let Some(order) = &root.order {
        validate_order_semantics(order, model)?;
    }
    match root.kind {
        RootKind::List => {
            root.filter.as_ref().ok_or_else(|| {
                ClientCompileError::manifest(
                    "client.manifest.root_filter",
                    format!("list root `{}` requires filter semantics", root.name),
                )
            })?;
            root.order.as_ref().ok_or_else(|| {
                ClientCompileError::manifest(
                    "client.manifest.root_order",
                    format!("list root `{}` requires order semantics", root.name),
                )
            })?;
            validate_pagination(
                root.pagination.as_ref().ok_or_else(|| {
                    ClientCompileError::manifest(
                        "client.manifest.root_pagination",
                        format!("list root `{}` requires pagination semantics", root.name),
                    )
                })?,
                &format!("root `{}`", root.name),
            )?;
            if root.aggregate.is_some() {
                return Err(ClientCompileError::manifest(
                    "client.manifest.root_aggregate",
                    format!(
                        "list root `{}` cannot declare aggregate semantics",
                        root.name
                    ),
                ));
            }
        }
        RootKind::ByPk => {
            if root.pagination.is_some()
                || root.aggregate.is_some()
                || root.filter.is_some()
                || root.order.is_some()
            {
                return Err(ClientCompileError::manifest(
                    "client.manifest.by_pk_semantics",
                    format!(
                        "by-pk root `{}` cannot declare filter, order, pagination, or aggregate semantics",
                        root.name
                    ),
                ));
            }
            if root.arguments.iter().any(|argument| {
                argument.kind != ManifestArgumentKind::PrimaryKey
                    || argument.nullable
                    || argument.list
            }) {
                return Err(ClientCompileError::manifest(
                    "client.manifest.by_pk_arguments",
                    format!(
                        "by-pk root `{}` may contain only non-null scalar primary-key arguments",
                        root.name
                    ),
                ));
            }
        }
        RootKind::Aggregate => {
            if root.pagination.is_some() || root.order.is_some() {
                return Err(ClientCompileError::manifest(
                    "client.manifest.aggregate_root_semantics",
                    format!(
                        "aggregate root `{}` cannot declare order or pagination semantics",
                        root.name
                    ),
                ));
            }
            validate_aggregate_semantics(
                root.aggregate.as_ref().ok_or_else(|| {
                    ClientCompileError::manifest(
                        "client.manifest.aggregate_root",
                        format!(
                            "aggregate root `{}` requires aggregate semantics",
                            root.name
                        ),
                    )
                })?,
                model,
            )?;
        }
    }
    Ok(())
}

fn validate_unique_arguments(
    root: &ManifestRoot,
    scalar_codecs: &BTreeMap<String, String>,
) -> Result<(), ClientCompileError> {
    validate_unique_arguments_for(
        &root.arguments,
        scalar_codecs,
        &format!("manifest root `{}`", root.name),
    )
}

fn validate_unique_arguments_for(
    arguments: &[ManifestArgument],
    scalar_codecs: &BTreeMap<String, String>,
    owner: &str,
) -> Result<(), ClientCompileError> {
    let mut names = BTreeSet::new();
    let mut kinds = BTreeSet::new();
    for argument in arguments {
        validate_graphql_name(&argument.name, "manifest argument")?;
        validate_graphql_name(&argument.type_name, "manifest argument type")?;
        if !names.insert(argument.name.as_str()) {
            return Err(ClientCompileError::manifest(
                "client.manifest.duplicate_argument",
                format!("{owner} repeats argument `{}`", argument.name),
            ));
        }
        if argument.kind != ManifestArgumentKind::PrimaryKey && !kinds.insert(argument.kind) {
            return Err(ClientCompileError::manifest(
                "client.manifest.duplicate_argument_kind",
                format!("{owner} repeats {:?} argument semantics", argument.kind),
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
                    "{owner} pagination argument `{}` must use scalar Int",
                    argument.name
                ),
            ));
        }
        match (scalar_codecs.get(&argument.type_name), &argument.codec) {
            (Some(expected), Some(actual)) if actual == expected => {}
            (Some(expected), Some(actual)) => {
                return Err(ClientCompileError::manifest(
                    "client.manifest.argument_codec",
                    format!(
                        "{owner} argument `{}` codec `{actual}` does not match scalar `{}` inventory codec `{expected}`",
                        argument.name, argument.type_name
                    ),
                ));
            }
            (Some(_), None) => {
                return Err(ClientCompileError::manifest(
                    "client.manifest.argument_codec",
                    format!(
                        "{owner} scalar argument `{}` is missing its codec",
                        argument.name
                    ),
                ));
            }
            (None, Some(actual)) => {
                return Err(ClientCompileError::manifest(
                    "client.manifest.argument_codec",
                    format!(
                        "{owner} argument `{}` declares codec `{actual}` for non-scalar type `{}`",
                        argument.name, argument.type_name
                    ),
                ));
            }
            (None, None) => {}
        }
    }
    Ok(())
}

fn canonicalize_projectors(projectors: &mut [ManifestProjector]) -> Result<(), ClientCompileError> {
    for projector in projectors.iter_mut() {
        canonicalize_string_set(
            &mut projector.facts,
            &format!("projector `{}` fact", projector.name),
        )?;
        canonicalize_string_set(
            &mut projector.models,
            &format!("projector `{}` model", projector.name),
        )?;
        canonicalize_string_set(
            &mut projector.dependencies,
            &format!("projector `{}` dependency", projector.name),
        )?;
    }
    projectors.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(())
}

fn validate_projectors(
    projectors: &[ManifestProjector],
    models: &BTreeMap<String, ManifestModel>,
) -> Result<(), ClientCompileError> {
    let mut names = BTreeSet::new();
    for projector in projectors {
        if projector.version != 1 {
            return Err(ClientCompileError::manifest(
                "client.manifest.projector_version",
                format!("projector `{}` must use version 1", projector.name),
            ));
        }
        validate_nonempty(&projector.name, "manifest projector name")?;
        if !names.insert(projector.name.as_str()) {
            return Err(ClientCompileError::manifest(
                "client.manifest.duplicate_projector",
                format!("duplicate manifest projector `{}`", projector.name),
            ));
        }
        if projector.facts.is_empty() || projector.models.is_empty() {
            return Err(ClientCompileError::manifest(
                "client.manifest.projector_inventory",
                format!(
                    "projector `{}` must declare at least one fact and model",
                    projector.name
                ),
            ));
        }
        let mut expected_dependencies = BTreeSet::new();
        for model in &projector.models {
            let Some(model_contract) = models.get(model) else {
                return Err(ClientCompileError::manifest(
                    "client.manifest.projector_model",
                    format!(
                        "projector `{}` references absent model `{model}`",
                        projector.name
                    ),
                ));
            };
            expected_dependencies.insert(model_contract.source_table.as_str());
        }
        let actual_dependencies = projector
            .dependencies
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        if actual_dependencies != expected_dependencies {
            return Err(ClientCompileError::manifest(
                "client.manifest.projector_dependency",
                format!(
                    "projector `{}` dependencies must exactly cover its model source tables",
                    projector.name
                ),
            ));
        }
    }
    Ok(())
}

fn validate_direct_projections(
    commands: &[ManifestCommand],
    models: &BTreeMap<String, ManifestModel>,
    projectors: &[ManifestProjector],
) -> Result<BTreeSet<String>, ClientCompileError> {
    let mut requiring_revalidation = BTreeSet::new();
    for command in commands {
        let consistency = command
            .extensions
            .consistency
            .as_ref()
            .map(|consistency| consistency.kind);
        let direct = command.extensions.direct_projection.as_ref();
        match (consistency, direct) {
            (Some(ManifestConsistencyKind::Projected), None) => {
                return Err(ClientCompileError::manifest(
                    "client.manifest.direct_projection_required",
                    format!(
                        "manifest projected command `{}` requires exactly one direct_projection target",
                        command.name
                    ),
                ));
            }
            (Some(ManifestConsistencyKind::Projected), Some(direct)) => {
                if validate_direct_projection(command, direct, models, projectors)? {
                    requiring_revalidation.insert(command.name.clone());
                }
            }
            (_, Some(_)) => {
                return Err(ClientCompileError::manifest(
                    "client.manifest.direct_projection_unexpected",
                    format!(
                        "manifest non-projected command `{}` cannot declare direct_projection",
                        command.name
                    ),
                ));
            }
            (_, None) => {}
        }
    }
    Ok(requiring_revalidation)
}

fn validate_direct_projection(
    command: &ManifestCommand,
    direct: &ManifestDirectProjection,
    models: &BTreeMap<String, ManifestModel>,
    projectors: &[ManifestProjector],
) -> Result<bool, ClientCompileError> {
    if direct.topology.version != 1 {
        return Err(ClientCompileError::manifest(
            "client.manifest.direct_projection_topology",
            format!(
                "manifest command `{}` direct projection topology version must be 1",
                command.name
            ),
        ));
    }
    validate_projection_name(
        &direct.topology.name,
        &format!(
            "manifest command `{}` direct projection topology name",
            command.name
        ),
    )?;
    validate_hash(
        &direct.topology.digest,
        &format!(
            "manifest command `{}` direct projection topology digest",
            command.name
        ),
    )?;
    validate_projection_epoch(
        &direct.change_epoch,
        &format!(
            "manifest command `{}` direct projection change_epoch",
            command.name
        ),
    )?;
    validate_graphql_name(
        &direct.model,
        &format!(
            "manifest command `{}` direct projection model",
            command.name
        ),
    )?;
    let model = models.get(&direct.model).ok_or_else(|| {
        ClientCompileError::manifest(
            "client.manifest.direct_projection_model",
            format!(
                "manifest command `{}` direct projection references absent model `{}`",
                command.name, direct.model
            ),
        )
    })?;
    if !matches!(
        &model.normalization,
        ManifestNormalization::Normalized { .. }
    ) {
        return Err(ClientCompileError::manifest(
            "client.manifest.direct_projection_model",
            format!(
                "manifest command `{}` direct projection model `{}` has no complete authorized normalized identity",
                command.name, direct.model
            ),
        ));
    }
    let ManifestCommandShape::Object { definition } = &command.output else {
        return Err(ClientCompileError::manifest(
            "client.manifest.direct_projection_output",
            format!(
                "manifest projected command `{}` must return its exact model object",
                command.name
            ),
        ));
    };
    if definition.name != model.typename || definition.fields.len() != model.fields.len() {
        return Err(ClientCompileError::manifest(
            "client.manifest.direct_projection_output",
            format!(
                "manifest projected command `{}` output `{}` does not exactly match model `{}` typename `{}`",
                command.name, definition.name, direct.model, model.typename
            ),
        ));
    }
    for field in &definition.fields {
        let matches = model.fields.iter().any(|model_field| {
            field.name == model_field.name
                && field.type_name == model_field.scalar
                && field.nullable == model_field.nullable
                && !field.list
                && !field.item_nullable
                && field.codec.as_deref() == Some(model_field.codec.as_str())
                && field.nested.is_none()
        });
        if !matches {
            return Err(ClientCompileError::manifest(
                "client.manifest.direct_projection_output",
                format!(
                    "manifest projected command `{}` output field `{}.{}` differs from model `{}`",
                    command.name, definition.name, field.name, direct.model
                ),
            ));
        }
    }

    let visible_owners = projectors
        .iter()
        .filter(|projector| projector.models.iter().any(|model| model == &direct.model))
        .collect::<Vec<_>>();
    match visible_owners.as_slice() {
        [] => {
            if projectors
                .iter()
                .any(|projector| projector.name == direct.topology.name)
            {
                return Err(ClientCompileError::manifest(
                    "client.manifest.direct_projection_owner",
                    format!(
                        "manifest command `{}` topology `{}` does not own model `{}`",
                        command.name, direct.topology.name, direct.model
                    ),
                ));
            }
            // A role surface may intentionally omit the topology because it
            // also owns denied models. The exact digest is sufficient and does
            // not disclose those hidden model/fact/table identities.
        }
        [owner] if owner.name == direct.topology.name => {}
        [owner] => {
            return Err(ClientCompileError::manifest(
                "client.manifest.direct_projection_owner",
                format!(
                    "manifest command `{}` names topology `{}` but visible projector `{}` owns model `{}`",
                    command.name, direct.topology.name, owner.name, direct.model
                ),
            ));
        }
        owners => {
            return Err(ClientCompileError::manifest(
                "client.manifest.direct_projection_owner",
                format!(
                    "manifest command `{}` model `{}` has ambiguous visible projector ownership: {}",
                    command.name,
                    direct.model,
                    owners
                        .iter()
                        .map(|owner| owner.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }
    }

    direct
        .partition
        .as_ref()
        .map(|partition| validate_direct_projection_partition(command, partition))
        .transpose()
        .map(|requires_revalidation| requires_revalidation.unwrap_or(false))
}

fn validate_projection_name(value: &str, label: &str) -> Result<(), ClientCompileError> {
    validate_nonempty(value, label)?;
    if value.len() > 128
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(ClientCompileError::manifest(
            "client.manifest.direct_projection_topology",
            format!("{label} must be at most 128 bytes without whitespace or control characters"),
        ));
    }
    Ok(())
}

fn validate_projection_epoch(value: &str, label: &str) -> Result<(), ClientCompileError> {
    validate_nonempty(value, label)?;
    if value.len() > 128 || value.chars().any(char::is_control) {
        return Err(ClientCompileError::manifest(
            "client.manifest.direct_projection_epoch",
            format!("{label} must be at most 128 bytes without control characters"),
        ));
    }
    Ok(())
}

fn validate_direct_projection_partition(
    command: &ManifestCommand,
    partition: &ManifestEffectExpression,
) -> Result<bool, ClientCompileError> {
    match partition {
        ManifestEffectExpression::Input { path } => {
            if path.is_empty() {
                return Err(ClientCompileError::manifest(
                    "client.manifest.direct_projection_partition",
                    format!(
                        "manifest command `{}` direct projection partition input path must not be empty",
                        command.name
                    ),
                ));
            }
            validate_nonempty_strings(
                path,
                &format!(
                    "manifest command `{}` direct projection partition input path",
                    command.name
                ),
            )?;
            let ManifestCommandShape::Object { definition } = &command.input else {
                return Err(ClientCompileError::manifest(
                    "client.manifest.direct_projection_partition",
                    format!(
                        "manifest command `{}` direct projection partition input requires a typed object input",
                        command.name
                    ),
                ));
            };
            let mut current = definition;
            for (index, segment) in path.iter().enumerate() {
                let field = current
                    .fields
                    .iter()
                    .find(|field| field.name == *segment)
                    .ok_or_else(|| {
                        ClientCompileError::manifest(
                            "client.manifest.direct_projection_partition",
                            format!(
                                "manifest command `{}` direct projection references unknown input path `{}`",
                                command.name,
                                path.join(".")
                            ),
                        )
                    })?;
                if index + 1 == path.len() {
                    if field.list || field.nested.is_some() {
                        return Err(ClientCompileError::manifest(
                            "client.manifest.direct_projection_partition",
                            format!(
                                "manifest command `{}` direct projection partition path `{}` must resolve to a scalar",
                                command.name,
                                path.join(".")
                            ),
                        ));
                    }
                    return Ok(false);
                }
                if field.list {
                    return Err(ClientCompileError::manifest(
                        "client.manifest.direct_projection_partition",
                        format!(
                            "manifest command `{}` direct projection partition path `{}` descends through a list",
                            command.name,
                            path.join(".")
                        ),
                    ));
                }
                current = field.nested.as_deref().ok_or_else(|| {
                    ClientCompileError::manifest(
                        "client.manifest.direct_projection_partition",
                        format!(
                            "manifest command `{}` direct projection partition path `{}` descends through a scalar",
                            command.name,
                            path.join(".")
                        ),
                    )
                })?;
            }
            unreachable!("non-empty direct projection path resolves or returns an error")
        }
        ManifestEffectExpression::TrustedPreset { name } => {
            let declared = command
                .extensions
                .trusted_presets
                .iter()
                .any(|descriptor| descriptor.name == *name && descriptor.codec == "string");
            if !declared {
                return Err(ClientCompileError::manifest(
                    "client.manifest.direct_projection_partition",
                    format!(
                        "manifest command `{}` direct projection partition trusted preset `{name}` must declare the string codec",
                        command.name
                    ),
                ));
            }
            Ok(false)
        }
        ManifestEffectExpression::Constant { .. } | ManifestEffectExpression::Null => Ok(false),
    }
}

fn canonicalize_commands(commands: &mut [ManifestCommand]) -> Result<(), ClientCompileError> {
    for command in commands.iter_mut() {
        canonicalize_string_set(
            &mut command.grants,
            &format!("command `{}` grant", command.name),
        )?;
        for descriptor in &command.extensions.trusted_presets {
            validate_nonempty(&descriptor.name, "trusted preset name")?;
            validate_nonempty(&descriptor.codec, "trusted preset codec")?;
            if descriptor.name.len() > 128
                || descriptor.name.trim() != descriptor.name
                || descriptor.name.chars().any(char::is_control)
            {
                return Err(ClientCompileError::manifest(
                    "client.manifest.trusted_preset_name",
                    format!(
                        "manifest command `{}` has an invalid trusted preset name",
                        command.name
                    ),
                ));
            }
        }
        command.extensions.trusted_presets.sort();
        if command
            .extensions
            .trusted_presets
            .windows(2)
            .any(|pair| pair[0].name == pair[1].name)
        {
            return Err(ClientCompileError::manifest(
                "client.manifest.trusted_preset_inventory",
                format!(
                    "manifest command `{}` repeats a trusted preset descriptor",
                    command.name
                ),
            ));
        }
        if let Some(defaults) = &mut command.extensions.input_defaults {
            for default in &mut defaults.defaults {
                validate_nonempty_strings(
                    &default.path,
                    &format!("command `{}` input default path", command.name),
                )?;
            }
            defaults.defaults.sort();
            if defaults
                .defaults
                .windows(2)
                .any(|pair| pair[0].path == pair[1].path)
            {
                return Err(ClientCompileError::manifest(
                    "client.manifest.input_default_path",
                    format!(
                        "manifest command `{}` repeats an input default path",
                        command.name
                    ),
                ));
            }
        }
        if let Some(direct) = &mut command.extensions.direct_projection {
            if let Some(partition) = &mut direct.partition {
                canonicalize_effect_expression(partition);
            }
        }
        if let Some(effects) = &mut command.extensions.effects {
            for effect in &mut effects.operations {
                canonicalize_effect(effect);
            }
        }
        if let Some(confirmations) = &mut command.extensions.confirmations {
            for confirmation in &mut confirmations.expected {
                canonicalize_effect_key(&mut confirmation.key);
                if let Some(partition) = &mut confirmation.partition {
                    canonicalize_effect_expression(partition);
                }
            }
        }
    }
    commands.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(())
}

fn canonicalize_effect(effect: &mut ManifestEffect) {
    match effect {
        ManifestEffect::Upsert { key, fields, .. } | ManifestEffect::Patch { key, fields, .. } => {
            canonicalize_effect_key(key);
            for field in fields.iter_mut() {
                canonicalize_effect_expression(&mut field.value);
            }
            fields.sort_by(|left, right| left.field.cmp(&right.field));
        }
        ManifestEffect::Delete { key, .. } => canonicalize_effect_key(key),
        ManifestEffect::Link { source, target, .. }
        | ManifestEffect::Unlink { source, target, .. } => {
            canonicalize_effect_key(source);
            canonicalize_effect_key(target);
        }
        ManifestEffect::InvalidateRelationship { source, .. } => {
            canonicalize_effect_key(source);
        }
        ManifestEffect::InvalidateModel { .. } => {}
    }
}

fn canonicalize_effect_key(key: &mut ManifestEffectKey) {
    for field in &mut key.fields {
        canonicalize_effect_expression(&mut field.value);
    }
}

fn canonicalize_effect_expression(expression: &mut ManifestEffectExpression) {
    if let ManifestEffectExpression::Constant { value } = expression {
        *value = canonical_json_value(std::mem::take(value));
    }
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
