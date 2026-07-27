use serde::{Deserialize, Serialize};

use crate::graphql::client_manifest::DISTRIBUTED_CLIENT_PROTOCOL_VERSION;

use super::OpaqueProtocolToken;

/// Stable command lifecycle vocabulary exposed to generated clients.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DistributedCommandState {
    InProgress,
    Accepted,
    AcceptedPendingProjection,
    Projected,
    Rejected,
    ProjectionFailed,
    Expired,
    Unknown,
}

/// Typed Rust consistency guarantee, serialized independently of domain data.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DistributedCommandConsistency {
    Accepted,
    Fact,
    Projected,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DistributedProjectionExpectation {
    pub(crate) projection: String,
    pub(crate) model: String,
    pub(crate) scope_token: OpaqueProtocolToken,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DistributedCommandMetadata {
    pub(crate) command_id: String,
    pub(crate) causation_id: String,
    pub(crate) state: DistributedCommandState,
    pub(crate) consistency: DistributedCommandConsistency,
    pub(crate) expects: Vec<DistributedProjectionExpectation>,
    /// Exact finite obligations already observed for this authorized command.
    /// Tokens are identical to their matching `expects` entry, so the client
    /// can retire optimism without reconstructing any hidden scope material.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) observations: Vec<DistributedProjectionObservation>,
    /// Exact same-transaction revision evidence retained in the durable replay
    /// envelope. Async commands leave this empty and confirm through expects.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) records: Vec<DistributedRecordRevision>,
}

/// Comparable revision for one record in a GraphQL response.
///
/// `scopeToken` is stable across operations and revisions but contains no
/// model key, tenant, topology, or partition plaintext. Positions are decimal
/// strings so JavaScript never coerces a future 64-bit value through `number`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DistributedRecordRevision {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) path: Option<Vec<String>>,
    pub(crate) model: String,
    pub(crate) scope_token: OpaqueProtocolToken,
    pub(crate) incarnation: String,
    pub(crate) revision: String,
    pub(crate) tombstone: bool,
}

/// One causation observation whose scope token is byte-for-byte comparable to
/// the corresponding command receipt obligation token.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DistributedProjectionObservation {
    pub(crate) causation_id: String,
    pub(crate) projection: String,
    pub(crate) model: String,
    pub(crate) scope_token: OpaqueProtocolToken,
}

/// Client-returned live cursor. `projection` selects one finite, role-visible
/// static projector scope; the token authenticates every hidden scope field,
/// operation instance, generation, and position.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DistributedLiveCursor {
    pub(crate) projection: String,
    pub(crate) position: String,
    pub(crate) token: OpaqueProtocolToken,
}

/// Bounded interpretation of the optional client resume extension.
///
/// Malformed or unverifiable input is intentionally represented as `Invalid`
/// rather than exposed as a request error: live execution must fall back to a
/// fresh snapshot and must never merge against an untrusted cursor.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum RequestedLiveResume {
    #[default]
    Absent,
    Invalid,
    Cursors(Vec<DistributedLiveCursor>),
}

/// Comparable position for one projector/partition member of an exact query
/// index snapshot. Positions from different `scopeToken`s are incomparable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DistributedIndexRevision {
    pub(crate) projection: String,
    pub(crate) scope_token: OpaqueProtocolToken,
    pub(crate) position: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) resume: Option<DistributedLiveCursor>,
}

/// Evidence attached to one exact operation instance.
///
/// These two completeness dimensions are deliberately independent:
///
/// - `recordsComplete=false` means some returned normalized record lacks
///   usable causal revision evidence.
/// - `indexesComparable=false` means the server cannot expose one safe,
///   complete projector-position vector for this authorized query.
///
/// A row-filtered query can therefore return complete authorized record
/// evidence while keeping its partition-wide index positions private. Clients
/// may treat that server-authorized payload as exact renderable membership, but
/// must not use it for causal index comparison, live handoff/resume, local
/// membership proofs, observations, or optimistic confirmation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DistributedQuerySnapshot {
    pub(crate) scope_token: OpaqueProtocolToken,
    pub(crate) records_complete: bool,
    pub(crate) indexes_comparable: bool,
    pub(crate) records: Vec<DistributedRecordRevision>,
    pub(crate) indexes: Vec<DistributedIndexRevision>,
    pub(crate) observations: Vec<DistributedProjectionObservation>,
}

impl DistributedQuerySnapshot {
    pub(super) fn discard_incomparable_index_evidence(&mut self) {
        if self.indexes_comparable {
            return;
        }
        self.indexes.clear();
        self.observations.clear();
    }
}

/// Per-frame resumability decision for a live operation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DistributedLiveMetadata {
    pub(crate) supported: bool,
    pub(crate) reset: bool,
    pub(crate) cursors: Vec<DistributedLiveCursor>,
}

/// One server-derived value for a descriptor already present in the selected
/// static client surface. Values are valid only under this envelope's exact
/// cache scope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DistributedTrustedPreset {
    pub(crate) name: String,
    pub(crate) codec: String,
    pub(crate) value: serde_json::Value,
}

/// Canonical contents of GraphQL's top-level `extensions.distributed`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DistributedEnvelopeV1 {
    pub(crate) protocol_version: u32,
    pub(crate) schema_hash: String,
    pub(crate) cache_scope: OpaqueProtocolToken,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) operation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) command: Option<DistributedCommandMetadata>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) snapshot: Option<DistributedQuerySnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) live: Option<DistributedLiveMetadata>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) trusted_presets: Vec<DistributedTrustedPreset>,
}

impl DistributedEnvelopeV1 {
    pub(crate) fn new(
        schema_hash: impl Into<String>,
        cache_scope: OpaqueProtocolToken,
        operation: Option<String>,
    ) -> Self {
        Self {
            protocol_version: DISTRIBUTED_CLIENT_PROTOCOL_VERSION,
            schema_hash: schema_hash.into(),
            cache_scope,
            operation,
            command: None,
            snapshot: None,
            live: None,
            trusted_presets: Vec::new(),
        }
    }

    pub(crate) fn with_trusted_presets(
        mut self,
        trusted_presets: Vec<DistributedTrustedPreset>,
    ) -> Self {
        self.trusted_presets = trusted_presets;
        self
    }
}
