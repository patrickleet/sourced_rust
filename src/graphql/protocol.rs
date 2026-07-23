//! Versioned framework metadata carried in GraphQL response extensions.
//!
//! Domain payloads remain ordinary GraphQL data. This module owns the one
//! `extensions.distributed` wire envelope plus opaque, keyed tokens for
//! authorization and projection scopes. Tokens are comparable capabilities,
//! never client-decodable identities and never bearer credentials.

use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex};

use async_graphql::{Response, Value};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use super::client_manifest::DISTRIBUTED_CLIENT_PROTOCOL_VERSION;
use super::command_contract::CommandConsistency;
use crate::command_ledger::CommandLedgerState;
use crate::microsvc::{
    CausalCommandProjectionObligation, CausalCommandPublicState, CausalCommandPublicStatus,
    CausalCommandReceiptSource, CausalProjectionEvidenceState,
};

const TOKEN_FORMAT_VERSION: &str = "v1";
const TOKEN_MAC_BYTES: usize = 32;
const TOKEN_DOMAIN: &[u8] = b"distributed.graphql.protocol-token";
const MAX_TOKEN_MATERIAL_BYTES: usize = 1024 * 1024;
const MAX_OPAQUE_TOKEN_BYTES: usize = 128;

/// Maximum resumable projector partitions carried in one live operation.
/// Request parsing and response generation share this bound so the server
/// never emits a cursor set that a conforming client must reject.
pub(crate) const MAX_LIVE_RESUME_CURSORS: usize = 64;

/// One server-owned token. Its string contents have no public structure.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct OpaqueProtocolToken(String);

impl OpaqueProtocolToken {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// Parse one bounded framework token supplied back by a client.
    ///
    /// This validates only the canonical outer representation. Callers must
    /// still verify its purpose and server-owned material before using it.
    pub(crate) fn parse(value: &str) -> Result<Self, ProtocolTokenError> {
        if value.is_empty() || value.len() > MAX_OPAQUE_TOKEN_BYTES {
            return Err(ProtocolTokenError::Malformed);
        }
        let mut segments = value.split('.');
        let version = segments.next().ok_or(ProtocolTokenError::Malformed)?;
        let purpose = segments.next().ok_or(ProtocolTokenError::Malformed)?;
        let encoded_mac = segments.next().ok_or(ProtocolTokenError::Malformed)?;
        if segments.next().is_some()
            || version != TOKEN_FORMAT_VERSION
            || ProtocolTokenPurpose::from_label(purpose).is_none()
        {
            return Err(ProtocolTokenError::Malformed);
        }
        let supplied = URL_SAFE_NO_PAD
            .decode(encoded_mac)
            .map_err(|_| ProtocolTokenError::Malformed)?;
        if supplied.len() != TOKEN_MAC_BYTES || URL_SAFE_NO_PAD.encode(&supplied) != encoded_mac {
            return Err(ProtocolTokenError::Malformed);
        }
        Ok(Self(value.to_string()))
    }
}

impl fmt::Debug for OpaqueProtocolToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OpaqueProtocolToken([redacted])")
    }
}

/// Domain separation for tokens that are intentionally not interchangeable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProtocolTokenPurpose {
    CacheScope,
    ProjectionObligation,
    ProjectionObservation,
    RecordRevision,
    QuerySnapshot,
    QueryIndex,
    LiveResume,
}

impl ProtocolTokenPurpose {
    const fn label(self) -> &'static str {
        match self {
            Self::CacheScope => "cache-scope",
            Self::ProjectionObligation => "projection-obligation",
            Self::ProjectionObservation => "projection-observation",
            Self::RecordRevision => "record-revision",
            Self::QuerySnapshot => "query-snapshot",
            Self::QueryIndex => "query-index",
            Self::LiveResume => "live-resume",
        }
    }

    fn from_label(value: &str) -> Option<Self> {
        match value {
            "cache-scope" => Some(Self::CacheScope),
            "projection-obligation" => Some(Self::ProjectionObligation),
            "projection-observation" => Some(Self::ProjectionObservation),
            "record-revision" => Some(Self::RecordRevision),
            "query-snapshot" => Some(Self::QuerySnapshot),
            "query-index" => Some(Self::QueryIndex),
            "live-resume" => Some(Self::LiveResume),
            _ => None,
        }
    }
}

/// Stable deployment key for deterministic opaque protocol tokens.
///
/// The key is deliberately exact-sized and redacted from `Debug`. Deployments
/// must preserve it across replicas and restarts whenever they want existing
/// cache/resume tokens to remain comparable.
#[derive(Clone)]
pub(crate) struct ProtocolTokenCodec {
    key: [u8; 32],
}

impl ProtocolTokenCodec {
    pub(crate) fn new(key: [u8; 32]) -> Self {
        Self { key }
    }

    /// Mint a deterministic token from a canonical serialization.
    ///
    /// Callers must use structs, tuples, ordered maps, or already-canonical
    /// protocol values. Unordered application maps are not accepted protocol
    /// material.
    pub(crate) fn issue<T: Serialize>(
        &self,
        purpose: ProtocolTokenPurpose,
        material: &T,
    ) -> Result<OpaqueProtocolToken, ProtocolTokenError> {
        let bytes =
            serde_json::to_vec(material).map_err(|_| ProtocolTokenError::InvalidMaterial)?;
        self.issue_bytes(purpose, &bytes)
    }

    pub(crate) fn issue_bytes(
        &self,
        purpose: ProtocolTokenPurpose,
        canonical_material: &[u8],
    ) -> Result<OpaqueProtocolToken, ProtocolTokenError> {
        if canonical_material.is_empty() || canonical_material.len() > MAX_TOKEN_MATERIAL_BYTES {
            return Err(ProtocolTokenError::InvalidMaterial);
        }
        let digest = self.mac(purpose, canonical_material);
        Ok(OpaqueProtocolToken(format!(
            "{TOKEN_FORMAT_VERSION}.{}.{}",
            purpose.label(),
            URL_SAFE_NO_PAD.encode(digest)
        )))
    }

    /// Verify a token against the expected purpose and canonical material.
    ///
    /// Tokens carry no plaintext payload, so successful verification proves
    /// only equality with server-owned expected material.
    pub(crate) fn verify<T: Serialize>(
        &self,
        token: &OpaqueProtocolToken,
        purpose: ProtocolTokenPurpose,
        material: &T,
    ) -> Result<(), ProtocolTokenError> {
        let bytes =
            serde_json::to_vec(material).map_err(|_| ProtocolTokenError::InvalidMaterial)?;
        self.verify_bytes(token, purpose, &bytes)
    }

    pub(crate) fn verify_bytes(
        &self,
        token: &OpaqueProtocolToken,
        purpose: ProtocolTokenPurpose,
        canonical_material: &[u8],
    ) -> Result<(), ProtocolTokenError> {
        if canonical_material.is_empty() || canonical_material.len() > MAX_TOKEN_MATERIAL_BYTES {
            return Err(ProtocolTokenError::InvalidMaterial);
        }
        let mut segments = token.as_str().split('.');
        let version = segments.next().ok_or(ProtocolTokenError::Malformed)?;
        let encoded_purpose = segments.next().ok_or(ProtocolTokenError::Malformed)?;
        let encoded_mac = segments.next().ok_or(ProtocolTokenError::Malformed)?;
        if segments.next().is_some()
            || version != TOKEN_FORMAT_VERSION
            || encoded_purpose != purpose.label()
        {
            return Err(ProtocolTokenError::Malformed);
        }
        let supplied = URL_SAFE_NO_PAD
            .decode(encoded_mac)
            .map_err(|_| ProtocolTokenError::Malformed)?;
        if supplied.len() != TOKEN_MAC_BYTES {
            return Err(ProtocolTokenError::Malformed);
        }
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&self.key).expect("HMAC-SHA256 accepts a 32-byte key");
        update_mac(&mut mac, purpose, canonical_material);
        mac.verify_slice(&supplied)
            .map_err(|_| ProtocolTokenError::Mismatch)
    }

    fn mac(&self, purpose: ProtocolTokenPurpose, material: &[u8]) -> [u8; TOKEN_MAC_BYTES] {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&self.key).expect("HMAC-SHA256 accepts a 32-byte key");
        update_mac(&mut mac, purpose, material);
        mac.finalize().into_bytes().into()
    }
}

impl fmt::Debug for ProtocolTokenCodec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProtocolTokenCodec([redacted])")
    }
}

fn update_mac(mac: &mut Hmac<Sha256>, purpose: ProtocolTokenPurpose, material: &[u8]) {
    mac.update(TOKEN_DOMAIN);
    mac.update(&DISTRIBUTED_CLIENT_PROTOCOL_VERSION.to_be_bytes());
    update_segment(mac, purpose.label().as_bytes());
    update_segment(mac, material);
}

fn update_segment(mac: &mut Hmac<Sha256>, value: &[u8]) {
    mac.update(&(value.len() as u64).to_be_bytes());
    mac.update(value);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProtocolTokenError {
    InvalidMaterial,
    Malformed,
    Mismatch,
}

impl fmt::Display for ProtocolTokenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidMaterial => "invalid protocol token material",
            Self::Malformed => "malformed protocol token",
            Self::Mismatch => "protocol token does not match the expected scope",
        })
    }
}

impl std::error::Error for ProtocolTokenError {}

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

#[derive(Serialize)]
struct LiveResumeTokenMaterial<'a> {
    domain: &'static str,
    version: u32,
    cache_scope: &'a str,
    schema_hash: &'a str,
    snapshot_scope: &'a str,
    projection: &'a str,
    topology: &'a crate::projection_protocol::ProjectorTopologyId,
    partition: &'a crate::projection_protocol::ProjectionPartition,
    epoch: &'a str,
    position: String,
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
/// `complete=false` is the explicit conservative fallback for legacy data,
/// mixed ownership, or a projector partition that a query cannot derive. The
/// client may consume safe record evidence but must revalidate the index.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DistributedQuerySnapshot {
    pub(crate) scope_token: OpaqueProtocolToken,
    pub(crate) complete: bool,
    pub(crate) records: Vec<DistributedRecordRevision>,
    pub(crate) indexes: Vec<DistributedIndexRevision>,
    pub(crate) observations: Vec<DistributedProjectionObservation>,
}

/// Per-frame resumability decision for a live operation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DistributedLiveMetadata {
    pub(crate) supported: bool,
    pub(crate) reset: bool,
    pub(crate) cursors: Vec<DistributedLiveCursor>,
}

/// Canonical contents of GraphQL's top-level `extensions.distributed`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DistributedEnvelopeV2 {
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
}

impl DistributedEnvelopeV2 {
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
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ProtocolResponseAccumulator {
    inner: Arc<ProtocolResponseState>,
}

#[derive(Debug)]
struct ProtocolResponseState {
    envelope: Mutex<DistributedEnvelopeV2>,
    query_snapshot_scope: Mutex<Option<OpaqueProtocolToken>>,
    requested_live_resume: Mutex<RequestedLiveResume>,
    /// `None` is one-shot HTTP execution. `Some` is a stream FIFO containing
    /// one immutable envelope per yielded GraphQL response.
    stream_frames: Mutex<Option<VecDeque<DistributedEnvelopeV2>>>,
    dispatch_claimed: Mutex<bool>,
    codec: ProtocolTokenCodec,
}

impl ProtocolResponseAccumulator {
    pub(crate) fn new(envelope: DistributedEnvelopeV2, codec: ProtocolTokenCodec) -> Self {
        Self {
            inner: Arc::new(ProtocolResponseState {
                envelope: Mutex::new(envelope),
                query_snapshot_scope: Mutex::new(None),
                requested_live_resume: Mutex::new(RequestedLiveResume::Absent),
                stream_frames: Mutex::new(None),
                dispatch_claimed: Mutex::new(false),
                codec,
            }),
        }
    }

    pub(crate) fn set_requested_live_resume(
        &self,
        requested: RequestedLiveResume,
    ) -> Result<(), ProtocolAccumulatorError> {
        *self
            .inner
            .requested_live_resume
            .lock()
            .map_err(|_| ProtocolAccumulatorError::Poisoned)? = requested;
        Ok(())
    }

    pub(crate) fn requested_live_resume(
        &self,
    ) -> Result<RequestedLiveResume, ProtocolAccumulatorError> {
        self.inner
            .requested_live_resume
            .lock()
            .map(|requested| requested.clone())
            .map_err(|_| ProtocolAccumulatorError::Poisoned)
    }

    /// Switch this request accumulator to stream mode before injecting it into
    /// async-graphql. Producers then enqueue immutable frame envelopes; the
    /// transport pops them in order and cannot race a later frame mutation.
    pub(crate) fn begin_stream(&self) -> Result<(), ProtocolAccumulatorError> {
        let mut frames = self
            .inner
            .stream_frames
            .lock()
            .map_err(|_| ProtocolAccumulatorError::Poisoned)?;
        if frames.is_none() {
            *frames = Some(VecDeque::new());
        }
        Ok(())
    }

    /// Stable operation-instance scope used by query/index evidence.
    ///
    /// Callers pass only canonical, bounded material (normally the generated
    /// operation identity plus canonical variables/window). Authorization and
    /// schema generation are always injected here and cannot be forgotten.
    pub(crate) fn issue_query_snapshot_scope<T: Serialize>(
        &self,
        operation_instance: &T,
    ) -> Result<OpaqueProtocolToken, ProtocolAccumulatorError> {
        #[derive(Serialize)]
        struct Material<'a, T> {
            domain: &'static str,
            version: u32,
            cache_scope: &'a str,
            schema_hash: &'a str,
            operation_instance: &'a T,
        }

        self.with_envelope(|envelope| {
            self.inner
                .codec
                .issue(
                    ProtocolTokenPurpose::QuerySnapshot,
                    &Material {
                        domain: "distributed.graphql.query-snapshot",
                        version: 1,
                        cache_scope: envelope.cache_scope.as_str(),
                        schema_hash: &envelope.schema_hash,
                        operation_instance,
                    },
                )
                .map_err(|_| ProtocolAccumulatorError::Encoding)
        })?
    }

    pub(crate) fn bind_query_snapshot_scope<T: Serialize>(
        &self,
        operation_instance: &T,
    ) -> Result<OpaqueProtocolToken, ProtocolAccumulatorError> {
        let token = self.issue_query_snapshot_scope(operation_instance)?;
        let mut existing = self
            .inner
            .query_snapshot_scope
            .lock()
            .map_err(|_| ProtocolAccumulatorError::Poisoned)?;
        match existing.as_ref() {
            None => *existing = Some(token.clone()),
            Some(existing) if existing == &token => {}
            Some(_) => return Err(ProtocolAccumulatorError::IncomparableSnapshot),
        }
        Ok(token)
    }

    pub(crate) fn query_snapshot_scope(
        &self,
    ) -> Result<OpaqueProtocolToken, ProtocolAccumulatorError> {
        self.inner
            .query_snapshot_scope
            .lock()
            .map_err(|_| ProtocolAccumulatorError::Poisoned)?
            .clone()
            .ok_or(ProtocolAccumulatorError::MissingSnapshotScope)
    }

    /// Stable record identity token. Revision numbers and operation identity
    /// are deliberately excluded so values from separate queries remain
    /// comparable only after this token matches.
    pub(crate) fn issue_record_scope(
        &self,
        scope: &crate::projection_protocol::ProjectionRecordScope,
    ) -> Result<OpaqueProtocolToken, ProtocolAccumulatorError> {
        #[derive(Serialize)]
        struct Material<'a> {
            domain: &'static str,
            version: u32,
            cache_scope: &'a str,
            schema_hash: &'a str,
            scope: &'a crate::projection_protocol::ProjectionRecordScope,
        }

        self.with_envelope(|envelope| {
            self.inner
                .codec
                .issue(
                    ProtocolTokenPurpose::RecordRevision,
                    &Material {
                        domain: "distributed.graphql.record-scope",
                        version: 1,
                        cache_scope: envelope.cache_scope.as_str(),
                        schema_hash: &envelope.schema_hash,
                        scope,
                    },
                )
                .map_err(|_| ProtocolAccumulatorError::Encoding)
        })?
    }

    /// Stable exact-index scope. The current head is excluded so later
    /// positions within the same projector/partition/query window compare.
    pub(crate) fn issue_index_scope(
        &self,
        snapshot_scope: &OpaqueProtocolToken,
        cursor: &crate::projection_protocol::ProjectionChangeCursor,
    ) -> Result<OpaqueProtocolToken, ProtocolAccumulatorError> {
        self.issue_index_scope_parts(
            snapshot_scope,
            cursor.topology(),
            cursor.projection_partition(),
            cursor.epoch(),
        )
    }

    pub(crate) fn issue_index_scope_parts(
        &self,
        snapshot_scope: &OpaqueProtocolToken,
        topology: &crate::projection_protocol::ProjectorTopologyId,
        partition: &crate::projection_protocol::ProjectionPartition,
        epoch: &crate::projection_protocol::ProjectionEpoch,
    ) -> Result<OpaqueProtocolToken, ProtocolAccumulatorError> {
        #[derive(Serialize)]
        struct Material<'a> {
            domain: &'static str,
            version: u32,
            cache_scope: &'a str,
            schema_hash: &'a str,
            snapshot_scope: &'a str,
            topology: &'a crate::projection_protocol::ProjectorTopologyId,
            partition: &'a crate::projection_protocol::ProjectionPartition,
            epoch: &'a str,
        }

        self.with_envelope(|envelope| {
            self.inner
                .codec
                .issue(
                    ProtocolTokenPurpose::QueryIndex,
                    &Material {
                        domain: "distributed.graphql.query-index",
                        version: 1,
                        cache_scope: envelope.cache_scope.as_str(),
                        schema_hash: &envelope.schema_hash,
                        snapshot_scope: snapshot_scope.as_str(),
                        topology,
                        partition,
                        epoch: epoch.as_str(),
                    },
                )
                .map_err(|_| ProtocolAccumulatorError::Encoding)
        })?
    }

    pub(crate) fn issue_live_resume(
        &self,
        projection: &str,
        snapshot_scope: &OpaqueProtocolToken,
        cursor: &crate::projection_protocol::ProjectionChangeCursor,
    ) -> Result<DistributedLiveCursor, ProtocolAccumulatorError> {
        self.issue_live_resume_position(
            projection,
            snapshot_scope,
            cursor.topology(),
            cursor.projection_partition(),
            cursor.epoch(),
            cursor.position(),
        )
    }

    pub(crate) fn issue_live_resume_position(
        &self,
        projection: &str,
        snapshot_scope: &OpaqueProtocolToken,
        topology: &crate::projection_protocol::ProjectorTopologyId,
        partition: &crate::projection_protocol::ProjectionPartition,
        epoch: &crate::projection_protocol::ProjectionEpoch,
        position: u64,
    ) -> Result<DistributedLiveCursor, ProtocolAccumulatorError> {
        let token = self.live_resume_token(
            projection,
            snapshot_scope,
            topology,
            partition,
            epoch,
            position,
        )?;
        Ok(DistributedLiveCursor {
            projection: projection.to_string(),
            position: position.to_string(),
            token,
        })
    }

    /// Verify a client-returned cursor against one server-derived static
    /// projector scope. The public projection/position fields select the
    /// finite candidate; hidden topology/partition/epoch remain MAC-bound.
    pub(crate) fn verify_live_resume(
        &self,
        supplied: &DistributedLiveCursor,
        snapshot_scope: &OpaqueProtocolToken,
        expected: &crate::projection_protocol::ProjectionChangeCursor,
    ) -> Result<(), ProtocolTokenError> {
        if supplied.position != expected.position().to_string() {
            return Err(ProtocolTokenError::Mismatch);
        }
        self.verify_live_resume_position(
            supplied,
            snapshot_scope,
            &supplied.projection,
            expected.topology(),
            expected.projection_partition(),
            expected.epoch(),
            expected.position(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn verify_live_resume_position(
        &self,
        supplied: &DistributedLiveCursor,
        snapshot_scope: &OpaqueProtocolToken,
        projection: &str,
        topology: &crate::projection_protocol::ProjectorTopologyId,
        partition: &crate::projection_protocol::ProjectionPartition,
        epoch: &crate::projection_protocol::ProjectionEpoch,
        position: u64,
    ) -> Result<(), ProtocolTokenError> {
        if supplied.projection != projection || supplied.position != position.to_string() {
            return Err(ProtocolTokenError::Mismatch);
        }
        self.with_envelope(|envelope| {
            self.inner.codec.verify(
                &supplied.token,
                ProtocolTokenPurpose::LiveResume,
                &LiveResumeTokenMaterial {
                    domain: "distributed.graphql.live-resume",
                    version: 1,
                    cache_scope: envelope.cache_scope.as_str(),
                    schema_hash: &envelope.schema_hash,
                    snapshot_scope: snapshot_scope.as_str(),
                    projection,
                    topology,
                    partition,
                    epoch: epoch.as_str(),
                    position: position.to_string(),
                },
            )
        })
        .map_err(|_| ProtocolTokenError::InvalidMaterial)?
    }

    fn live_resume_token(
        &self,
        projection: &str,
        snapshot_scope: &OpaqueProtocolToken,
        topology: &crate::projection_protocol::ProjectorTopologyId,
        partition: &crate::projection_protocol::ProjectionPartition,
        epoch: &crate::projection_protocol::ProjectionEpoch,
        position: u64,
    ) -> Result<OpaqueProtocolToken, ProtocolAccumulatorError> {
        self.with_envelope(|envelope| {
            self.inner
                .codec
                .issue(
                    ProtocolTokenPurpose::LiveResume,
                    &LiveResumeTokenMaterial {
                        domain: "distributed.graphql.live-resume",
                        version: 1,
                        cache_scope: envelope.cache_scope.as_str(),
                        schema_hash: &envelope.schema_hash,
                        snapshot_scope: snapshot_scope.as_str(),
                        projection,
                        topology,
                        partition,
                        epoch: epoch.as_str(),
                        position: position.to_string(),
                    },
                )
                .map_err(|_| ProtocolAccumulatorError::Encoding)
        })?
    }

    /// Issue the exact token used by both receipt expectations and query/live
    /// observations. Equality is therefore meaningful without exposing or
    /// reconstructing canonical partition/key bytes in JavaScript.
    pub(crate) fn issue_projection_obligation_scope(
        &self,
        causation_id: &str,
        projector: &str,
        model: &str,
        observation_kind: crate::projection_protocol::ProjectionObservationKind,
        scope: &crate::projection_protocol::ProjectionRecordScope,
    ) -> Result<OpaqueProtocolToken, ProtocolAccumulatorError> {
        #[derive(Serialize)]
        struct TokenMaterial<'a> {
            domain: &'static str,
            version: u32,
            cache_scope: &'a str,
            schema_hash: &'a str,
            causation_id: &'a str,
            projector: &'a str,
            model: &'a str,
            observation_kind: &'static str,
            scope: &'a crate::projection_protocol::ProjectionRecordScope,
        }

        self.with_envelope(|envelope| {
            self.inner
                .codec
                .issue(
                    ProtocolTokenPurpose::ProjectionObligation,
                    &TokenMaterial {
                        domain: "distributed.graphql.projection-obligation",
                        version: 1,
                        cache_scope: envelope.cache_scope.as_str(),
                        schema_hash: &envelope.schema_hash,
                        causation_id,
                        projector,
                        model,
                        observation_kind: observation_kind.as_storage_str(),
                        scope,
                    },
                )
                .map_err(|_| ProtocolAccumulatorError::Encoding)
        })?
    }

    fn with_envelope<T>(
        &self,
        operation: impl FnOnce(&DistributedEnvelopeV2) -> T,
    ) -> Result<T, ProtocolAccumulatorError> {
        self.inner
            .envelope
            .lock()
            .map(|envelope| operation(&envelope))
            .map_err(|_| ProtocolAccumulatorError::Poisoned)
    }

    /// Reserve this operation's single causal-command dispatch slot before
    /// invoking application code. This prevents a multi-field mutation from
    /// committing a second causal command and only then discovering that one
    /// response envelope cannot represent both receipts.
    pub(crate) fn claim_dispatch(&self) -> Result<(), ProtocolAccumulatorError> {
        let mut claimed = self
            .inner
            .dispatch_claimed
            .lock()
            .map_err(|_| ProtocolAccumulatorError::Poisoned)?;
        if *claimed {
            return Err(ProtocolAccumulatorError::MultipleCommands);
        }
        *claimed = true;
        Ok(())
    }

    /// Convert exact durable replay material into the public wire receipt.
    ///
    /// The command payload is deliberately not consumed here. Canonical
    /// projector partition/key material is fed only into HMAC token issuance
    /// and can never appear in the serialized envelope.
    pub(crate) fn record_receipt(
        &self,
        receipt: &CausalCommandReceiptSource,
    ) -> Result<(), ProtocolAccumulatorError> {
        let observed = (receipt.state == CommandLedgerState::Projected)
            .then(|| (0..receipt.obligations.len()).collect::<Vec<_>>())
            .unwrap_or_default();
        self.record_command(self.metadata(
            &receipt.command_id,
            &receipt.causation_id,
            command_state(receipt.state),
            receipt.consistency,
            &receipt.obligations,
            receipt.direct_projection.as_ref(),
            &observed,
        )?)
    }

    /// Record the authorized public status when it contains a complete receipt.
    ///
    /// `unknown` and compact `expired` status intentionally omit command
    /// metadata: their GraphQL data state remains useful without fabricating or
    /// disclosing causation. `in_progress` is complete because the durable
    /// reservation already has a causation ID and consistency contract.
    pub(crate) fn record_status(
        &self,
        status: &CausalCommandPublicStatus,
    ) -> Result<(), ProtocolAccumulatorError> {
        let (Some(causation_id), Some(consistency)) =
            (status.causation_id.as_deref(), status.consistency)
        else {
            return Ok(());
        };
        let observed = status
            .evidence
            .iter()
            .filter(|evidence| evidence.state == CausalProjectionEvidenceState::Observed)
            .map(|evidence| evidence.obligation_index)
            .collect::<Vec<_>>();
        self.record_command(self.metadata(
            &status.command_id,
            causation_id,
            public_command_state(status.state),
            consistency,
            &status.obligations,
            status.direct_projection.as_ref(),
            &observed,
        )?)
    }

    fn metadata(
        &self,
        command_id: &str,
        causation_id: &str,
        state: DistributedCommandState,
        consistency: CommandConsistency,
        obligations: &[CausalCommandProjectionObligation],
        direct_projection: Option<&crate::projection_protocol::SameTransactionProjectionEvidence>,
        observed_obligation_indices: &[usize],
    ) -> Result<DistributedCommandMetadata, ProtocolAccumulatorError> {
        let expects = obligations
            .iter()
            .map(|obligation| {
                self.issue_projection_obligation_scope(
                    causation_id,
                    &obligation.projector,
                    &obligation.model,
                    obligation.observation_kind,
                    &obligation.scope,
                )
                .map(|scope_token| DistributedProjectionExpectation {
                    projection: obligation.projector.clone(),
                    model: obligation.model.clone(),
                    scope_token,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut seen_observations = std::collections::BTreeSet::new();
        let observations = observed_obligation_indices
            .iter()
            .map(|index| {
                if !seen_observations.insert(*index) {
                    return Err(ProtocolAccumulatorError::Encoding);
                }
                let obligation = obligations
                    .get(*index)
                    .ok_or(ProtocolAccumulatorError::Encoding)?;
                Ok(DistributedProjectionObservation {
                    causation_id: causation_id.to_string(),
                    projection: obligation.projector.clone(),
                    model: obligation.model.clone(),
                    scope_token: self.issue_projection_obligation_scope(
                        causation_id,
                        &obligation.projector,
                        &obligation.model,
                        obligation.observation_kind,
                        &obligation.scope,
                    )?,
                })
            })
            .collect::<Result<Vec<_>, ProtocolAccumulatorError>>()?;
        let records = direct_projection
            .into_iter()
            .flat_map(|evidence| evidence.records.iter())
            .map(|record| {
                Ok(DistributedRecordRevision {
                    path: None,
                    model: record.revision.scope().model().to_string(),
                    scope_token: self.issue_record_scope(record.revision.scope())?,
                    incarnation: record.revision.incarnation().to_string(),
                    revision: record.revision.revision().to_string(),
                    tombstone: record.tombstone,
                })
            })
            .collect::<Result<Vec<_>, ProtocolAccumulatorError>>()?;

        Ok(DistributedCommandMetadata {
            command_id: command_id.to_string(),
            causation_id: causation_id.to_string(),
            state,
            consistency: command_consistency(consistency),
            expects,
            observations,
            records,
        })
    }

    /// Record the one generated causal command represented by this operation.
    /// Re-recording identical metadata is idempotent; a second distinct command
    /// fails closed rather than attaching an ambiguous receipt.
    pub(crate) fn record_command(
        &self,
        command: DistributedCommandMetadata,
    ) -> Result<(), ProtocolAccumulatorError> {
        let mut envelope = self
            .inner
            .envelope
            .lock()
            .map_err(|_| ProtocolAccumulatorError::Poisoned)?;
        match &envelope.command {
            None => envelope.command = Some(command),
            Some(existing) if existing == &command => {}
            Some(_) => return Err(ProtocolAccumulatorError::MultipleCommands),
        }
        Ok(())
    }

    /// Attach query/live evidence to this HTTP response or enqueue one
    /// immutable GraphQL-WS frame envelope.
    pub(crate) fn record_query_metadata(
        &self,
        snapshot: DistributedQuerySnapshot,
        live: Option<DistributedLiveMetadata>,
    ) -> Result<(), ProtocolAccumulatorError> {
        let mut envelope = self
            .inner
            .envelope
            .lock()
            .map_err(|_| ProtocolAccumulatorError::Poisoned)?;
        let mut frames = self
            .inner
            .stream_frames
            .lock()
            .map_err(|_| ProtocolAccumulatorError::Poisoned)?;

        if let Some(frames) = frames.as_mut() {
            let mut frame = envelope.clone();
            frame.snapshot = Some(snapshot);
            frame.live = live;
            frames.push_back(frame);
            return Ok(());
        }

        match &mut envelope.snapshot {
            None => envelope.snapshot = Some(snapshot),
            Some(existing) if existing.scope_token == snapshot.scope_token => {
                existing.complete &= snapshot.complete;
                existing.records.extend(snapshot.records);
                existing.indexes.extend(snapshot.indexes);
                existing.observations.extend(snapshot.observations);
            }
            Some(_) => return Err(ProtocolAccumulatorError::IncomparableSnapshot),
        }
        match (&envelope.live, live) {
            (None, Some(live)) => envelope.live = Some(live),
            (Some(existing), Some(live)) if existing == &live => {}
            (Some(_), Some(_)) => return Err(ProtocolAccumulatorError::IncomparableSnapshot),
            (_, None) => {}
        }
        Ok(())
    }

    pub(crate) fn snapshot(&self) -> Result<DistributedEnvelopeV2, ProtocolAccumulatorError> {
        self.inner
            .envelope
            .lock()
            .map(|envelope| envelope.clone())
            .map_err(|_| ProtocolAccumulatorError::Poisoned)
    }

    pub(crate) fn attach(&self, response: &mut Response) -> Result<(), ProtocolAccumulatorError> {
        if response.extensions.contains_key("distributed") {
            return Err(ProtocolAccumulatorError::ExtensionCollision);
        }
        let envelope = {
            let envelope = self
                .inner
                .envelope
                .lock()
                .map_err(|_| ProtocolAccumulatorError::Poisoned)?;
            let mut frames = self
                .inner
                .stream_frames
                .lock()
                .map_err(|_| ProtocolAccumulatorError::Poisoned)?;
            match frames.as_mut() {
                Some(frames) => frames.pop_front().unwrap_or_else(|| envelope.clone()),
                None => envelope.clone(),
            }
        };
        let json =
            serde_json::to_value(envelope).map_err(|_| ProtocolAccumulatorError::Encoding)?;
        let value = Value::from_json(json).map_err(|_| ProtocolAccumulatorError::Encoding)?;
        response.extensions.insert("distributed".into(), value);
        Ok(())
    }
}

fn command_consistency(value: CommandConsistency) -> DistributedCommandConsistency {
    match value {
        CommandConsistency::Accepted => DistributedCommandConsistency::Accepted,
        CommandConsistency::Fact => DistributedCommandConsistency::Fact,
        CommandConsistency::Projected => DistributedCommandConsistency::Projected,
    }
}

fn command_state(value: CommandLedgerState) -> DistributedCommandState {
    match value {
        CommandLedgerState::InProgress | CommandLedgerState::RetryableUnknown => {
            DistributedCommandState::InProgress
        }
        CommandLedgerState::Accepted => DistributedCommandState::Accepted,
        CommandLedgerState::AcceptedPendingProjection => {
            DistributedCommandState::AcceptedPendingProjection
        }
        CommandLedgerState::Projected => DistributedCommandState::Projected,
        CommandLedgerState::Rejected => DistributedCommandState::Rejected,
        CommandLedgerState::ProjectionFailed => DistributedCommandState::ProjectionFailed,
        CommandLedgerState::Expired => DistributedCommandState::Expired,
    }
}

fn public_command_state(value: CausalCommandPublicState) -> DistributedCommandState {
    match value {
        CausalCommandPublicState::InProgress => DistributedCommandState::InProgress,
        CausalCommandPublicState::Accepted => DistributedCommandState::Accepted,
        CausalCommandPublicState::AcceptedPendingProjection => {
            DistributedCommandState::AcceptedPendingProjection
        }
        CausalCommandPublicState::Projected => DistributedCommandState::Projected,
        CausalCommandPublicState::Rejected => DistributedCommandState::Rejected,
        CausalCommandPublicState::ProjectionFailed => DistributedCommandState::ProjectionFailed,
        CausalCommandPublicState::Expired => DistributedCommandState::Expired,
        CausalCommandPublicState::Unknown => DistributedCommandState::Unknown,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProtocolAccumulatorError {
    Poisoned,
    MultipleCommands,
    ExtensionCollision,
    IncomparableSnapshot,
    MissingSnapshotScope,
    Encoding,
}

impl fmt::Display for ProtocolAccumulatorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Poisoned => "protocol response state is unavailable",
            Self::MultipleCommands => {
                "one GraphQL operation cannot carry multiple causal command receipts"
            }
            Self::ExtensionCollision => "GraphQL response already defines extensions.distributed",
            Self::IncomparableSnapshot => {
                "GraphQL operation produced incomparable query snapshot metadata"
            }
            Self::MissingSnapshotScope => "GraphQL query snapshot scope was not initialized",
            Self::Encoding => "protocol response encoding failed",
        })
    }
}

impl std::error::Error for ProtocolAccumulatorError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection_protocol::{
        ProjectionChange, ProjectionChangeCursor, ProjectionChangeKind, ProjectionEpoch,
        ProjectionObservation, ProjectionObservationKind, ProjectionPartition,
        ProjectionRecordMetadata, ProjectionRecordScope, ProjectorTopologyId, RecordRevision,
        SameTransactionProjectionEvidence,
    };

    fn codec(byte: u8) -> ProtocolTokenCodec {
        ProtocolTokenCodec::new([byte; 32])
    }

    #[test]
    fn opaque_tokens_are_deterministic_bound_and_non_disclosing() {
        let material = ("tenant-7", "todos", "private-key-42");
        let token = codec(7)
            .issue(ProtocolTokenPurpose::ProjectionObligation, &material)
            .unwrap();
        let again = codec(7)
            .issue(ProtocolTokenPurpose::ProjectionObligation, &material)
            .unwrap();
        assert_eq!(token, again);
        assert!(!token.as_str().contains("tenant"));
        assert!(!token.as_str().contains("private-key"));
        codec(7)
            .verify(
                &token,
                ProtocolTokenPurpose::ProjectionObligation,
                &material,
            )
            .unwrap();
        assert_eq!(
            codec(7).verify(&token, ProtocolTokenPurpose::CacheScope, &material),
            Err(ProtocolTokenError::Malformed)
        );
        assert_eq!(
            codec(8).verify(
                &token,
                ProtocolTokenPurpose::ProjectionObligation,
                &material
            ),
            Err(ProtocolTokenError::Mismatch)
        );
        assert_eq!(
            codec(7).verify(
                &token,
                ProtocolTokenPurpose::ProjectionObligation,
                &("tenant-7", "todos", "other")
            ),
            Err(ProtocolTokenError::Mismatch)
        );
        assert_eq!(format!("{token:?}"), "OpaqueProtocolToken([redacted])");
    }

    #[test]
    fn malformed_or_tampered_tokens_fail_closed() {
        let material = ("scope", 9_u64);
        let token = codec(3)
            .issue(ProtocolTokenPurpose::CacheScope, &material)
            .unwrap();
        let mut changed = token.as_str().as_bytes().to_vec();
        let last = changed.last_mut().unwrap();
        *last = if *last == b'A' { b'B' } else { b'A' };
        let tampered = OpaqueProtocolToken(String::from_utf8(changed).unwrap());
        assert!(matches!(
            codec(3).verify(&tampered, ProtocolTokenPurpose::CacheScope, &material),
            Err(ProtocolTokenError::Mismatch | ProtocolTokenError::Malformed)
        ));
        let malformed = OpaqueProtocolToken("scope-is-not-a-token".into());
        assert_eq!(
            codec(3).verify(&malformed, ProtocolTokenPurpose::CacheScope, &material),
            Err(ProtocolTokenError::Malformed)
        );
    }

    fn command(id: &str) -> DistributedCommandMetadata {
        DistributedCommandMetadata {
            command_id: id.into(),
            causation_id: "cause-17".into(),
            state: DistributedCommandState::AcceptedPendingProjection,
            consistency: DistributedCommandConsistency::Fact,
            expects: vec![DistributedProjectionExpectation {
                projection: "todos".into(),
                model: "TodoView".into(),
                scope_token: codec(9)
                    .issue(ProtocolTokenPurpose::ProjectionObligation, &(id, 42_u64))
                    .unwrap(),
            }],
            observations: Vec::new(),
            records: Vec::new(),
        }
    }

    fn receipt() -> CausalCommandReceiptSource {
        let topology = ProjectorTopologyId::new(1, "todos", [17; 32]).unwrap();
        let partition =
            ProjectionPartition::new(br#"["tenant-private-900719925474099312345"]"#.to_vec())
                .unwrap();
        let scope = ProjectionRecordScope::new(
            topology,
            partition,
            "TodoView",
            br#"["9223372036854775807","child-private"]"#.to_vec(),
        )
        .unwrap();
        CausalCommandReceiptSource {
            command_id: "0190a000-0000-7000-8000-000000000042".into(),
            causation_id: "0190a000-0000-7000-8000-000000000017".into(),
            consistency: CommandConsistency::Fact,
            state: CommandLedgerState::AcceptedPendingProjection,
            outcome: serde_json::json!({ "accepted": true }),
            obligations: vec![CausalCommandProjectionObligation {
                projector: "todos".into(),
                model: "TodoView".into(),
                scope,
                observation_kind: ProjectionObservationKind::Record,
            }],
            direct_projection: None,
        }
    }

    fn direct_projected_receipt() -> CausalCommandReceiptSource {
        let mut receipt = receipt();
        let scope = receipt.obligations[0].scope.clone();
        let revision = RecordRevision::new(scope.clone(), 3, 9_007_199_254_740_991).unwrap();
        let change = ProjectionChangeCursor::new(
            scope.topology().clone(),
            scope.projection_partition().clone(),
            ProjectionEpoch::new("direct-v1").unwrap(),
            17,
        )
        .unwrap();
        receipt.consistency = CommandConsistency::Projected;
        receipt.state = CommandLedgerState::Projected;
        receipt.obligations.clear();
        receipt.direct_projection = Some(SameTransactionProjectionEvidence {
            records: vec![ProjectionRecordMetadata {
                revision: revision.clone(),
                tombstone: false,
                change: change.clone(),
            }],
            changes: vec![ProjectionChange {
                cursor: change.clone(),
                kind: ProjectionChangeKind::RecordUpsert,
                causation_id: receipt.causation_id.clone(),
                observation_kind: None,
                scope: Some(scope.clone()),
                revision: Some(revision.clone()),
                failure_id: None,
            }],
            observations: vec![ProjectionObservation {
                causation_id: receipt.causation_id.clone(),
                kind: ProjectionObservationKind::Record,
                revision: Some(revision),
                scope,
                change,
            }],
        });
        receipt
    }

    fn change_cursor(position: u64) -> ProjectionChangeCursor {
        ProjectionChangeCursor::new(
            ProjectorTopologyId::new(1, "todos", [17; 32]).unwrap(),
            ProjectionPartition::new(br#"["tenant-private-900719925474099312345"]"#.to_vec())
                .unwrap(),
            ProjectionEpoch::new("todos-v1").unwrap(),
            position,
        )
        .unwrap()
    }

    fn query_snapshot(
        accumulator: &ProtocolResponseAccumulator,
        operation: &str,
        position: u64,
    ) -> DistributedQuerySnapshot {
        let scope_token = accumulator
            .issue_query_snapshot_scope(&(operation, "canonical-window"))
            .unwrap();
        let cursor = change_cursor(position);
        DistributedQuerySnapshot {
            scope_token: scope_token.clone(),
            complete: true,
            records: Vec::new(),
            indexes: vec![DistributedIndexRevision {
                projection: "todos".into(),
                scope_token: accumulator
                    .issue_index_scope(&scope_token, &cursor)
                    .unwrap(),
                position: position.to_string(),
                resume: Some(
                    accumulator
                        .issue_live_resume("todos", &scope_token, &cursor)
                        .unwrap(),
                ),
            }],
            observations: Vec::new(),
        }
    }

    fn accumulator(key: u8, cache_material: &str, schema: &str) -> ProtocolResponseAccumulator {
        let codec = codec(key);
        let cache_scope = codec
            .issue(ProtocolTokenPurpose::CacheScope, &cache_material)
            .unwrap();
        ProtocolResponseAccumulator::new(
            DistributedEnvelopeV2::new(schema, cache_scope, None),
            codec,
        )
    }

    #[test]
    fn accumulator_is_idempotent_and_rejects_ambiguous_receipts() {
        let scope = codec(9)
            .issue(ProtocolTokenPurpose::CacheScope, &("principal", "surface"))
            .unwrap();
        let accumulator = ProtocolResponseAccumulator::new(
            DistributedEnvelopeV2::new("sha256:schema", scope, Some("sha256:operation".into())),
            codec(9),
        );
        accumulator.claim_dispatch().unwrap();
        assert_eq!(
            accumulator.claim_dispatch(),
            Err(ProtocolAccumulatorError::MultipleCommands)
        );
        accumulator.record_command(command("cmd-1")).unwrap();
        accumulator.record_command(command("cmd-1")).unwrap();
        assert_eq!(
            accumulator.record_command(command("cmd-2")),
            Err(ProtocolAccumulatorError::MultipleCommands)
        );

        let envelope = accumulator.snapshot().unwrap();
        let json = serde_json::to_value(envelope).unwrap();
        assert_eq!(json["protocolVersion"], 2);
        assert_eq!(json["schemaHash"], "sha256:schema");
        assert_eq!(json["operation"], "sha256:operation");
        assert_eq!(json["command"]["commandId"], "cmd-1");
        assert_eq!(json["command"]["state"], "accepted_pending_projection");
        assert_eq!(json["command"]["expects"][0]["model"], "TodoView");
        assert!(json["command"]["expects"][0]["scopeToken"]
            .as_str()
            .unwrap()
            .starts_with("v1.projection-obligation."));
    }

    #[test]
    fn durable_receipts_issue_stable_generation_bound_non_disclosing_obligations() {
        let receipt = receipt();
        let first = accumulator(11, "principal-a", "sha256:schema-a");
        first.record_receipt(&receipt).unwrap();
        let first = serde_json::to_value(first.snapshot().unwrap()).unwrap();
        let first_token = first["command"]["expects"][0]["scopeToken"]
            .as_str()
            .unwrap();
        assert_eq!(first["command"]["state"], "accepted_pending_projection");
        assert_eq!(first["command"]["consistency"], "fact");
        assert!(!first_token.contains("tenant-private"));
        assert!(!first_token.contains("9223372036854775807"));
        assert!(!first_token.contains("child-private"));

        let replay = accumulator(11, "principal-a", "sha256:schema-a");
        replay.record_receipt(&receipt).unwrap();
        let replay = serde_json::to_value(replay.snapshot().unwrap()).unwrap();
        assert_eq!(
            first_token,
            replay["command"]["expects"][0]["scopeToken"]
                .as_str()
                .unwrap()
        );

        for changed in [
            accumulator(12, "principal-a", "sha256:schema-a"),
            accumulator(11, "principal-b", "sha256:schema-a"),
            accumulator(11, "principal-a", "sha256:schema-b"),
        ] {
            changed.record_receipt(&receipt).unwrap();
            let changed = serde_json::to_value(changed.snapshot().unwrap()).unwrap();
            assert_ne!(
                first_token,
                changed["command"]["expects"][0]["scopeToken"]
                    .as_str()
                    .unwrap()
            );
        }
    }

    #[test]
    fn direct_projected_receipt_replays_exact_record_revision_as_decimal_strings() {
        let receipt = direct_projected_receipt();
        let first = accumulator(17, "principal-a", "sha256:schema-a");
        first.record_receipt(&receipt).unwrap();
        let command = serde_json::to_value(first.snapshot().unwrap()).unwrap()["command"].clone();
        assert_eq!(command["state"], "projected");
        assert_eq!(command["consistency"], "projected");
        assert_eq!(command["records"][0]["incarnation"], "3");
        assert_eq!(command["records"][0]["revision"], "9007199254740991");
        assert_eq!(command["records"][0]["tombstone"], false);
        assert!(command["records"][0].get("path").is_none());
        let token = command["records"][0]["scopeToken"].as_str().unwrap();
        assert!(token.starts_with("v1.record-revision."));
        assert!(!token.contains("tenant-private"));
        assert!(!token.contains("child-private"));

        let replay = accumulator(17, "principal-a", "sha256:schema-a");
        replay.record_receipt(&receipt).unwrap();
        let replay = serde_json::to_value(replay.snapshot().unwrap()).unwrap();
        assert_eq!(command["records"], replay["command"]["records"]);
    }

    #[test]
    fn unknown_status_does_not_fabricate_receipt_identity() {
        let accumulator = accumulator(5, "principal-a", "sha256:schema-a");
        accumulator
            .record_status(&CausalCommandPublicStatus {
                state: CausalCommandPublicState::Unknown,
                command_id: "0190a000-0000-7000-8000-000000000099".into(),
                causation_id: None,
                consistency: None,
                outcome: None,
                obligations: Vec::new(),
                evidence: Vec::new(),
                direct_projection: None,
            })
            .unwrap();
        let envelope = serde_json::to_value(accumulator.snapshot().unwrap()).unwrap();
        assert!(envelope.get("command").is_none());
    }

    #[test]
    fn projected_status_exposes_only_matching_opaque_observations() {
        let source = receipt();
        let accumulator = accumulator(5, "principal-a", "sha256:schema-a");
        accumulator
            .record_status(&CausalCommandPublicStatus {
                state: CausalCommandPublicState::Projected,
                command_id: source.command_id,
                causation_id: Some(source.causation_id.clone()),
                consistency: Some(source.consistency),
                outcome: Some(source.outcome),
                obligations: source.obligations,
                evidence: vec![crate::microsvc::CausalCommandProjectionEvidence {
                    obligation_index: 0,
                    state: CausalProjectionEvidenceState::Observed,
                    incarnation: Some(1),
                    revision: Some(7),
                }],
                direct_projection: None,
            })
            .unwrap();
        let command =
            serde_json::to_value(accumulator.snapshot().unwrap()).unwrap()["command"].clone();
        assert_eq!(command["state"], "projected");
        assert_eq!(
            command["observations"][0]["causationId"],
            source.causation_id
        );
        assert_eq!(
            command["observations"][0]["scopeToken"],
            command["expects"][0]["scopeToken"]
        );
        let encoded = command.to_string();
        assert!(!encoded.contains("tenant-private"));
        assert!(!encoded.contains("9223372036854775807"));
    }

    #[test]
    fn revision_and_resume_tokens_are_scoped_comparable_and_tamper_evident() {
        let accumulator = accumulator(31, "principal-a", "sha256:schema-a");
        let snapshot_scope = accumulator
            .issue_query_snapshot_scope(&("sha256:operation-a", "window-a"))
            .unwrap();
        let other_operation = accumulator
            .issue_query_snapshot_scope(&("sha256:operation-b", "window-a"))
            .unwrap();
        assert_ne!(snapshot_scope, other_operation);

        let receipt = receipt();
        let scope = &receipt.obligations[0].scope;
        let record_scope = accumulator.issue_record_scope(scope).unwrap();
        assert_eq!(record_scope, accumulator.issue_record_scope(scope).unwrap());
        assert!(!record_scope.as_str().contains("tenant-private"));
        assert!(!record_scope.as_str().contains("9223372036854775807"));

        let cursor = change_cursor(9_007_199_254_740_991);
        let live = accumulator
            .issue_live_resume("todos", &snapshot_scope, &cursor)
            .unwrap();
        assert_eq!(live.position, "9007199254740991");
        accumulator
            .verify_live_resume(&live, &snapshot_scope, &cursor)
            .unwrap();

        let parsed = OpaqueProtocolToken::parse(live.token.as_str()).unwrap();
        assert_eq!(parsed, live.token);
        let mut wrong_position = live.clone();
        wrong_position.position = "9007199254740990".into();
        assert_eq!(
            accumulator.verify_live_resume(&wrong_position, &snapshot_scope, &cursor),
            Err(ProtocolTokenError::Mismatch)
        );
        assert_eq!(
            accumulator.verify_live_resume(&live, &other_operation, &cursor),
            Err(ProtocolTokenError::Mismatch)
        );
        assert_eq!(
            OpaqueProtocolToken::parse("v1.live-resume.not/canonical"),
            Err(ProtocolTokenError::Malformed)
        );
    }

    #[test]
    fn observation_tokens_exactly_match_receipt_obligations() {
        let receipt = receipt();
        let accumulator = accumulator(41, "principal-a", "sha256:schema-a");
        accumulator.record_receipt(&receipt).unwrap();
        let expectation = accumulator.snapshot().unwrap().command.unwrap().expects[0]
            .scope_token
            .clone();
        let obligation = &receipt.obligations[0];
        let observed = accumulator
            .issue_projection_obligation_scope(
                &receipt.causation_id,
                &obligation.projector,
                &obligation.model,
                obligation.observation_kind,
                &obligation.scope,
            )
            .unwrap();
        assert_eq!(expectation, observed);
    }

    #[test]
    fn stream_frames_are_immutable_fifo_and_do_not_bleed_forward() {
        let accumulator = accumulator(51, "principal-a", "sha256:schema-a");
        accumulator.begin_stream().unwrap();
        let first = query_snapshot(&accumulator, "operation-a", 1);
        let second = query_snapshot(&accumulator, "operation-a", 2);
        accumulator
            .record_query_metadata(
                first.clone(),
                Some(DistributedLiveMetadata {
                    supported: true,
                    reset: true,
                    cursors: vec![first.indexes[0].resume.clone().unwrap()],
                }),
            )
            .unwrap();
        accumulator
            .record_query_metadata(
                second.clone(),
                Some(DistributedLiveMetadata {
                    supported: true,
                    reset: false,
                    cursors: vec![second.indexes[0].resume.clone().unwrap()],
                }),
            )
            .unwrap();

        let mut first_response = Response::new(Value::Null);
        let mut second_response = Response::new(Value::Null);
        let mut trailing_response = Response::new(Value::Null);
        accumulator.attach(&mut first_response).unwrap();
        accumulator.attach(&mut second_response).unwrap();
        accumulator.attach(&mut trailing_response).unwrap();

        let first_json = first_response.extensions["distributed"]
            .clone()
            .into_json()
            .unwrap();
        let second_json = second_response.extensions["distributed"]
            .clone()
            .into_json()
            .unwrap();
        let trailing_json = trailing_response.extensions["distributed"]
            .clone()
            .into_json()
            .unwrap();
        assert_eq!(first_json["snapshot"]["indexes"][0]["position"], "1");
        assert_eq!(first_json["live"]["reset"], true);
        assert_eq!(second_json["snapshot"]["indexes"][0]["position"], "2");
        assert_eq!(second_json["live"]["reset"], false);
        assert!(trailing_json.get("snapshot").is_none());
        assert!(trailing_json.get("live").is_none());
    }
}
