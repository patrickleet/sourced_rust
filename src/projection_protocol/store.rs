//! Sealed projection commit vocabulary shared by repository adapters.
//!
//! Application projectors never construct these batches directly. A
//! framework-owned workspace validates scopes and stages row mutations, then
//! hands one closed batch to a repository. This keeps row data, dedupe,
//! revisions, observations, checkpoints, and change publication inside one
//! adapter transaction.

use std::fmt;
use std::future::Future;
use std::num::NonZeroU64;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    ProjectionChangeCursor, ProjectionCheckpoint, ProjectionCommitOutcome, ProjectionEpoch,
    ProjectionInputCursor, ProjectionPartition, ProjectionProtocolValidationError,
    ProjectionRecordScope, ProjectionScopeCodec, ProjectionSource, ProjectorTopologyId,
    RecordRevision, MAX_PROJECTION_PARTITION_BYTES, MAX_PROJECTION_POSITION,
    MAX_PROJECTION_RECORD_KEY_BYTES,
};
use crate::repository::{InboxReceipt, RepositoryError};
use crate::table::{
    RowKey, RowValues, TableMutation, TableSchema, TableStoreError, TableWritePlan,
};

const INPUT_FINGERPRINT_DOMAIN: &[u8] = b"distributed.projection.input.v1\0";
const FAILURE_FINGERPRINT_DOMAIN: &[u8] = b"distributed.projection.failure.v1\0";
const MAX_MESSAGE_ID_BYTES: usize = 255;
const MAX_CAUSATION_ID_BYTES: usize = 128;
const MAX_FAILURE_ID_BYTES: usize = 255;
const MAX_FAILURE_CODE_BYTES: usize = 255;
const MAX_FAILURE_DETAIL_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_PROJECTION_QUERY_CHECKPOINT_PROBES: usize = 128;
pub(crate) const MAX_PROJECTION_QUERY_BATCH_ROWS: usize = 4_096;
pub(crate) const MAX_PROJECTION_QUERY_BATCH_CHECKPOINT_PROBES: usize = 4_096;
pub(crate) const MAX_PROJECTION_EVIDENCE_BATCH_ITEMS: usize = 128;

/// Default maximum number of durable changes retained per projector partition.
pub const DEFAULT_MAX_RETAINED_PROJECTION_CHANGES: u64 = 4_096;

/// Automatic per-partition change-log retention bound.
///
/// Every change-producing transaction retains at most this many newest entries
/// and advances the durable compacted-through watermark only after the older
/// contiguous prefix is actually removed. The watermark remains authoritative
/// if configuration is later lengthened; compacted entries are never inferred
/// or advertised as restored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProjectionChangeRetention(NonZeroU64);

impl ProjectionChangeRetention {
    pub fn new(max_retained_changes: u64) -> Result<Self, ProjectionProtocolValidationError> {
        if max_retained_changes > MAX_PROJECTION_POSITION {
            return Err(ProjectionProtocolValidationError::TooLarge {
                field: "projection retained change count",
                value: max_retained_changes,
                max: MAX_PROJECTION_POSITION,
            });
        }
        NonZeroU64::new(max_retained_changes).map(Self).ok_or(
            ProjectionProtocolValidationError::Zero {
                field: "projection retained change count",
            },
        )
    }

    pub fn max_retained_changes(self) -> u64 {
        self.0.get()
    }
}

impl Default for ProjectionChangeRetention {
    fn default() -> Self {
        Self(
            NonZeroU64::new(DEFAULT_MAX_RETAINED_PROJECTION_CHANGES)
                .expect("the default projection change retention is nonzero"),
        )
    }
}

/// Explicit repair generation for one projector partition.
///
/// Generation one is the initial run. A terminal failure can only be retried
/// after an operator creates a later generation linked to the failed one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProjectionGeneration(NonZeroU64);

impl ProjectionGeneration {
    pub fn new(value: u64) -> Result<Self, ProjectionProtocolValidationError> {
        if value > MAX_PROJECTION_POSITION {
            return Err(ProjectionProtocolValidationError::TooLarge {
                field: "projection generation",
                value,
                max: MAX_PROJECTION_POSITION,
            });
        }
        NonZeroU64::new(value)
            .map(Self)
            .ok_or(ProjectionProtocolValidationError::Zero {
                field: "projection generation",
            })
    }

    pub fn initial() -> Self {
        Self(NonZeroU64::MIN)
    }

    pub fn get(self) -> u64 {
        self.0.get()
    }

    pub fn checked_next(self) -> Result<Self, ProjectionProtocolError> {
        let next = self
            .get()
            .checked_add(1)
            .ok_or(ProjectionProtocolError::PositionOverflow {
                domain: "projection generation",
            })?;
        Self::new(next).map_err(|_| ProjectionProtocolError::PositionOverflow {
            domain: "projection generation",
        })
    }
}

/// SHA-256 identity of the exact canonical projector input.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProjectionInputFingerprint([u8; 32]);

impl ProjectionInputFingerprint {
    /// Fingerprint bytes emitted by a registered source adapter after it has
    /// canonicalized every semantic part of the input envelope.
    pub fn from_canonical_bytes(bytes: &[u8]) -> Self {
        Self(domain_separated_digest(INPUT_FINGERPRINT_DOMAIN, bytes))
    }

    pub(crate) fn from_digest(digest: [u8; 32]) -> Self {
        Self(digest)
    }

    pub fn digest(self) -> [u8; 32] {
        self.0
    }
}

/// Framework-authenticated input used by the sealed asynchronous commit path.
///
/// Its constructor is crate-private by design. A public `Message` and its
/// metadata cannot mint ordering evidence; only a registered transport/source
/// adapter may do so after authenticating its cursor scope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TrustedProjectionInput {
    pub(crate) cursor: ProjectionInputCursor,
    pub(crate) fingerprint: ProjectionInputFingerprint,
    pub(crate) message_id: String,
    pub(crate) causation_id: String,
    pub(crate) generation: ProjectionGeneration,
    pub(crate) gap_free: bool,
}

impl TrustedProjectionInput {
    pub(crate) fn mint(
        cursor: ProjectionInputCursor,
        fingerprint: ProjectionInputFingerprint,
        message_id: impl Into<String>,
        causation_id: impl Into<String>,
        generation: ProjectionGeneration,
        gap_free: bool,
    ) -> Result<Self, ProjectionProtocolError> {
        let message_id = bounded_opaque("projection message ID", message_id, MAX_MESSAGE_ID_BYTES)?;
        let causation_id = bounded_opaque(
            "projection causation ID",
            causation_id,
            MAX_CAUSATION_ID_BYTES,
        )?;
        Ok(Self {
            cursor,
            fingerprint,
            message_id,
            causation_id,
            generation,
            gap_free,
        })
    }

    pub(crate) fn inbox_receipt(&self) -> InboxReceipt {
        InboxReceipt::new(self.consumer_name(), self.message_id.clone())
    }

    fn consumer_name(&self) -> String {
        format!(
            "projection:v1:{}:{}:{}",
            digest_hex(&self.cursor.topology().digest()),
            digest_hex(&self.cursor.projection_partition().digest()),
            self.generation.get(),
        )
    }
}

/// Read-only disposition of one exact framework-authenticated input.
///
/// Projector runtimes use this before typed decoding and handler invocation so
/// fan-out redelivery of an already committed sibling cannot rerun application
/// projection logic. `Pending` is only returned after exact immutable identity,
/// active generation, stop, source-capability, and repair fences pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ProjectionInputDisposition {
    Pending,
    Duplicate(ProjectionCheckpoint),
    Stale(ProjectionCheckpoint),
}

/// Fail-closed ownership registration for one projection model/table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProjectionModelOwnership {
    pub(crate) model: String,
    pub(crate) table: String,
}

impl ProjectionModelOwnership {
    pub(crate) fn new(
        model: impl Into<String>,
        table: impl Into<String>,
    ) -> Result<Self, ProjectionProtocolError> {
        Ok(Self {
            model: bounded_name("projection model", model, 255)?,
            table: bounded_name("projection table", table, 255)?,
        })
    }
}

/// Record state required before a staged mutation may apply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ProjectionRecordExpectation {
    Missing,
    Exact(RecordRevision),
}

/// Protocol meaning of a staged table mutation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProjectionMutationKind {
    Upsert,
    Delete,
    Recreate,
}

/// One scope-checked record mutation in a sealed projection batch.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ProjectionRecordMutation {
    pub(crate) scope: ProjectionRecordScope,
    pub(crate) mutation: TableMutation,
    pub(crate) expectation: ProjectionRecordExpectation,
    pub(crate) kind: ProjectionMutationKind,
}

impl ProjectionRecordMutation {
    pub(crate) fn new(
        scope: ProjectionRecordScope,
        mutation: TableMutation,
        expectation: ProjectionRecordExpectation,
        kind: ProjectionMutationKind,
    ) -> Result<Self, ProjectionProtocolError> {
        if let ProjectionRecordExpectation::Exact(revision) = &expectation {
            if revision.scope() != &scope {
                return Err(ProjectionProtocolError::ScopeMismatch {
                    field: "projection record expectation",
                });
            }
        }
        let is_delete = matches!(mutation, TableMutation::DeleteRow(_));
        if is_delete != matches!(kind, ProjectionMutationKind::Delete) {
            return Err(ProjectionProtocolError::InvalidBatch(
                "projection delete kind and table mutation disagree".into(),
            ));
        }
        if matches!(
            kind,
            ProjectionMutationKind::Delete | ProjectionMutationKind::Recreate
        ) && !matches!(expectation, ProjectionRecordExpectation::Exact(_))
        {
            return Err(ProjectionProtocolError::InvalidBatch(
                "projection delete/recreate requires an exact record revision".into(),
            ));
        }
        Ok(Self {
            scope,
            mutation,
            expectation,
            kind,
        })
    }
}

/// Whether an observation confirms a concrete row or a dependency scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProjectionObservationKind {
    Record,
    Dependency,
}

impl ProjectionObservationKind {
    pub(crate) fn as_storage_str(self) -> &'static str {
        match self {
            Self::Record => "record",
            Self::Dependency => "dependency",
        }
    }
}

/// Revision fence for one causation observation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ProjectionObservationTarget {
    /// Observe the revision allocated to a mutation of this scope in the batch.
    StagedRecord(ProjectionRecordScope),
    /// Observe an already persisted exact revision without mutating the row.
    ExistingRecord(RecordRevision),
    /// Observe an embedded/dependency scope that has no independent row or
    /// record revision. Clients revalidate its owning dependency when seen.
    Dependency(ProjectionRecordScope),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProjectionObservationRequest {
    pub(crate) kind: ProjectionObservationKind,
    pub(crate) target: ProjectionObservationTarget,
}

impl ProjectionObservationRequest {
    pub(crate) fn scope(&self) -> &ProjectionRecordScope {
        match &self.target {
            ProjectionObservationTarget::StagedRecord(scope)
            | ProjectionObservationTarget::Dependency(scope) => scope,
            ProjectionObservationTarget::ExistingRecord(revision) => revision.scope(),
        }
    }
}

/// One closed asynchronous projector transaction.
#[derive(Debug)]
pub(crate) struct ProjectionCommitBatch {
    pub(crate) input: TrustedProjectionInput,
    pub(crate) change_epoch: ProjectionEpoch,
    pub(crate) ownership: Vec<ProjectionModelOwnership>,
    pub(crate) mutations: Vec<ProjectionRecordMutation>,
    pub(crate) observations: Vec<ProjectionObservationRequest>,
}

impl ProjectionCommitBatch {
    pub(crate) fn validate(&self) -> Result<(), ProjectionProtocolError> {
        let topology = self.input.cursor.topology();
        let partition = self.input.cursor.projection_partition();
        self.input.inbox_receipt().validate()?;

        let mut owned_models = std::collections::HashMap::new();
        let mut owned_tables = std::collections::HashSet::new();
        for ownership in &self.ownership {
            if owned_models
                .insert(ownership.model.as_str(), ownership.table.as_str())
                .is_some()
                || !owned_tables.insert(ownership.table.as_str())
            {
                return Err(ProjectionProtocolError::InvalidBatch(format!(
                    "projection ownership repeats model `{}` or table `{}`",
                    ownership.model, ownership.table
                )));
            }
        }

        let mut scopes = std::collections::HashSet::new();
        for mutation in &self.mutations {
            validate_scope(topology, partition, &mutation.scope)?;
            let schema = match &mutation.mutation {
                TableMutation::UpsertRow(mutation) => mutation.schema,
                TableMutation::PatchRow(mutation) => mutation.schema,
                TableMutation::DeleteRow(mutation) => mutation.schema,
            };
            if mutation.scope.model() != schema.model_name
                || owned_models.get(mutation.scope.model()).copied()
                    != Some(schema.table_name.as_str())
            {
                return Err(ProjectionProtocolError::InvalidBatch(format!(
                    "projection record scope `{}` is not registered to table `{}`",
                    mutation.scope.model(),
                    schema.table_name
                )));
            }
            if !scopes.insert(mutation.scope.clone()) {
                return Err(ProjectionProtocolError::InvalidBatch(format!(
                    "projection batch repeats model `{}` record scope",
                    mutation.scope.model()
                )));
            }
        }
        TableWritePlan::new(
            self.mutations
                .iter()
                .map(|mutation| mutation.mutation.clone())
                .collect(),
        )
        .validate()?;

        let mut observations = std::collections::HashSet::new();
        for observation in &self.observations {
            validate_scope(topology, partition, observation.scope())?;
            if !owned_models.contains_key(observation.scope().model()) {
                return Err(ProjectionProtocolError::InvalidBatch(format!(
                    "projection observation scope `{}` is not registered",
                    observation.scope().model()
                )));
            }
            if !observations.insert((observation.kind, observation.scope().clone())) {
                return Err(ProjectionProtocolError::InvalidBatch(format!(
                    "projection batch repeats {:?} observation for model `{}`",
                    observation.kind,
                    observation.scope().model()
                )));
            }
            match (&observation.kind, &observation.target) {
                (
                    ProjectionObservationKind::Record,
                    ProjectionObservationTarget::StagedRecord(scope),
                ) if !scopes.contains(scope) => {
                    return Err(ProjectionProtocolError::InvalidBatch(format!(
                        "projection observation for model `{}` references an unstaged record",
                        scope.model()
                    )));
                }
                (
                    ProjectionObservationKind::Record,
                    ProjectionObservationTarget::StagedRecord(_)
                    | ProjectionObservationTarget::ExistingRecord(_),
                )
                | (
                    ProjectionObservationKind::Dependency,
                    ProjectionObservationTarget::Dependency(_),
                ) => {}
                _ => {
                    return Err(ProjectionProtocolError::InvalidBatch(
                        "projection observation kind and target disagree".into(),
                    ));
                }
            }
        }
        Ok(())
    }
}

/// One exact row mutation sealed out of a same-transaction command workspace.
///
/// Direct command projection deliberately has no caller-supplied record
/// expectation. The adapter must inspect the authoritative record metadata
/// while holding the registered projector partition lock: a missing row and
/// missing metadata creates revision one, a live row advances its revision,
/// and a tombstone or row/metadata disagreement fails closed.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SameTransactionProjectionMutation {
    pub(crate) scope: ProjectionRecordScope,
    pub(crate) mutation: TableMutation,
}

/// A sealed direct projection participant in one ledger-fenced domain commit.
///
/// This is intentionally a different type from [`ProjectionCommitBatch`].
/// Same-transaction projection has no source cursor, input fingerprint,
/// consumer inbox receipt, repair generation, or async checkpoint.
#[derive(Debug)]
pub(crate) struct SameTransactionProjectionBatch {
    pub(crate) topology: ProjectorTopologyId,
    pub(crate) partition: ProjectionPartition,
    pub(crate) change_epoch: ProjectionEpoch,
    pub(crate) ownership: Vec<ProjectionModelOwnership>,
    pub(crate) causation_id: String,
    pub(crate) mutations: Vec<SameTransactionProjectionMutation>,
    pub(crate) observations: Vec<ProjectionObservationRequest>,
}

impl SameTransactionProjectionBatch {
    pub(crate) fn single_upsert(
        topology: ProjectorTopologyId,
        partition: ProjectionPartition,
        change_epoch: ProjectionEpoch,
        ownership: ProjectionModelOwnership,
        scope: ProjectionRecordScope,
        mutation: TableMutation,
        causation_id: impl Into<String>,
    ) -> Result<Self, ProjectionProtocolError> {
        let observations = vec![ProjectionObservationRequest {
            kind: ProjectionObservationKind::Record,
            target: ProjectionObservationTarget::StagedRecord(scope.clone()),
        }];
        let batch = Self {
            topology,
            partition,
            change_epoch,
            ownership: vec![ownership],
            causation_id: bounded_opaque(
                "projection causation ID",
                causation_id,
                MAX_CAUSATION_ID_BYTES,
            )?,
            mutations: vec![SameTransactionProjectionMutation { scope, mutation }],
            observations,
        };
        batch.validate()?;
        Ok(batch)
    }

    pub(crate) fn validate(&self) -> Result<(), ProjectionProtocolError> {
        if self.mutations.len() != 1 || self.observations.len() != 1 {
            return Err(ProjectionProtocolError::InvalidBatch(
                "a direct projected command must contain exactly one row upsert and one exact record observation"
                    .into(),
            ));
        }
        if self.ownership.len() != 1 {
            return Err(ProjectionProtocolError::InvalidBatch(
                "a direct projected command must declare exactly one output model owner".into(),
            ));
        }
        bounded_opaque(
            "projection causation ID",
            self.causation_id.clone(),
            MAX_CAUSATION_ID_BYTES,
        )?;

        let ownership = &self.ownership[0];
        let staged = &self.mutations[0];
        validate_scope(&self.topology, &self.partition, &staged.scope)?;
        let TableMutation::UpsertRow(row) = &staged.mutation else {
            return Err(ProjectionProtocolError::InvalidBatch(
                "a direct projected command must seal one full-row upsert".into(),
            ));
        };
        if row.mode != crate::table::RowWriteMode::Upsert
            || row.expected_version != crate::table::ExpectedVersion::Any
        {
            return Err(ProjectionProtocolError::InvalidBatch(
                "a direct projected command requires an unfenced full-row upsert; the projection protocol owns its revision fence"
                    .into(),
            ));
        }
        if staged.scope.model() != row.schema.model_name
            || ownership.model != row.schema.model_name
            || ownership.table != row.schema.table_name
        {
            return Err(ProjectionProtocolError::InvalidBatch(format!(
                "direct projection scope/ownership does not match model `{}` table `{}`",
                row.schema.model_name, row.schema.table_name
            )));
        }
        TableWritePlan::new(vec![staged.mutation.clone()]).validate()?;

        let observation = &self.observations[0];
        match (&observation.kind, &observation.target) {
            (
                ProjectionObservationKind::Record,
                ProjectionObservationTarget::StagedRecord(scope),
            ) if scope == &staged.scope => Ok(()),
            _ => Err(ProjectionProtocolError::InvalidBatch(
                "a direct projected command must observe the exact staged record scope".into(),
            )),
        }
    }
}

/// Closed terminal-failure transaction for one exact input/generation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProjectionFailureBatch {
    pub(crate) input: TrustedProjectionInput,
    pub(crate) change_epoch: ProjectionEpoch,
    pub(crate) failure_id: String,
    pub(crate) failure_code: String,
    pub(crate) failure_bytes: Vec<u8>,
    pub(crate) failure_digest: [u8; 32],
}

impl ProjectionFailureBatch {
    /// Recompute the protocol fingerprint for persisted failure bytes.
    ///
    /// Storage adapters use this while decoding rows so corrupt bytes cannot
    /// be returned with an otherwise well-formed stored digest.
    pub(crate) fn fingerprint_bytes(failure_bytes: &[u8]) -> [u8; 32] {
        domain_separated_digest(FAILURE_FINGERPRINT_DOMAIN, failure_bytes)
    }

    pub(crate) fn new(
        input: TrustedProjectionInput,
        change_epoch: ProjectionEpoch,
        failure_id: impl Into<String>,
        failure_code: impl Into<String>,
        failure_bytes: impl Into<Vec<u8>>,
    ) -> Result<Self, ProjectionProtocolError> {
        let failure_bytes = failure_bytes.into();
        let failure_digest = Self::fingerprint_bytes(&failure_bytes);
        let batch = Self {
            input,
            change_epoch,
            failure_id: failure_id.into(),
            failure_code: failure_code.into(),
            failure_bytes,
            failure_digest,
        };
        batch.validate()?;
        Ok(batch)
    }

    pub(crate) fn validate(&self) -> Result<(), ProjectionProtocolError> {
        bounded_opaque(
            "projection failure ID",
            self.failure_id.clone(),
            MAX_FAILURE_ID_BYTES,
        )?;
        bounded_name(
            "projection failure code",
            self.failure_code.clone(),
            MAX_FAILURE_CODE_BYTES,
        )?;
        if self.failure_bytes.is_empty() || self.failure_bytes.len() > MAX_FAILURE_DETAIL_BYTES {
            return Err(ProjectionProtocolError::InvalidBatch(format!(
                "projection failure details must contain 1..={MAX_FAILURE_DETAIL_BYTES} bytes"
            )));
        }
        if Self::fingerprint_bytes(&self.failure_bytes) != self.failure_digest {
            return Err(ProjectionProtocolError::InvalidBatch(
                "projection failure digest does not match its exact bytes".into(),
            ));
        }
        bounded_opaque(
            "projection message ID",
            self.input.message_id.clone(),
            MAX_MESSAGE_ID_BYTES,
        )?;
        bounded_opaque(
            "projection causation ID",
            self.input.causation_id.clone(),
            MAX_CAUSATION_ID_BYTES,
        )?;
        self.input.inbox_receipt().validate()?;
        Ok(())
    }
}

/// Durable record metadata returned by projection stores and query snapshots.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectionRecordMetadata {
    pub revision: RecordRevision,
    pub tombstone: bool,
    pub change: ProjectionChangeCursor,
}

/// One exact input-source checkpoint requested alongside a physical query row.
///
/// The topology and partition are retained in every probe so a caller cannot
/// accidentally combine source progress from another projector scope. The
/// enclosing snapshot request validates those values before any adapter read.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ProjectionCheckpointProbe {
    pub(crate) topology: ProjectorTopologyId,
    pub(crate) partition: ProjectionPartition,
    pub(crate) source: ProjectionSource,
    pub(crate) epoch: ProjectionEpoch,
    pub(crate) generation: ProjectionGeneration,
}

impl ProjectionCheckpointProbe {
    pub(crate) fn new(
        topology: ProjectorTopologyId,
        partition: ProjectionPartition,
        source: ProjectionSource,
        epoch: ProjectionEpoch,
        generation: ProjectionGeneration,
    ) -> Self {
        Self {
            topology,
            partition,
            source,
            epoch,
            generation,
        }
    }
}

/// Sealed adapter-neutral request for one causally versioned physical row.
///
/// Construction derives `scope` from the topology's registered key codec.
/// Consequently the SQL/in-memory row key and projection metadata key cannot be
/// supplied independently. Task 16 can add its wire envelope around this
/// internal primitive without gaining authority to forge causal evidence.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ProjectionQuerySnapshotRequest {
    pub(crate) schema: Arc<TableSchema>,
    pub(crate) key: RowKey,
    pub(crate) scope: ProjectionRecordScope,
    pub(crate) checkpoint_probes: Vec<ProjectionCheckpointProbe>,
}

impl ProjectionQuerySnapshotRequest {
    pub(crate) fn new(
        codec: &ProjectionScopeCodec,
        partition: Option<&serde_json::Value>,
        model: &str,
        key: RowKey,
        checkpoint_probes: Vec<ProjectionCheckpointProbe>,
    ) -> Result<Self, ProjectionProtocolError> {
        let schema = codec.registered_schema_owned(model).map_err(|error| {
            ProjectionProtocolError::InvalidBatch(format!(
                "invalid projection query snapshot model: {error}"
            ))
        })?;
        let partition = codec.encode_partition(partition).map_err(|error| {
            ProjectionProtocolError::InvalidBatch(format!(
                "invalid projection query snapshot partition: {error}"
            ))
        })?;
        let scope = codec
            .encode_row_scope_in_partition(model, partition, &key)
            .map_err(|error| {
                ProjectionProtocolError::InvalidBatch(format!(
                    "invalid projection query snapshot key: {error}"
                ))
            })?;
        let request = Self {
            schema,
            key,
            scope,
            checkpoint_probes,
        };
        request.validate()?;
        Ok(request)
    }

    pub(crate) fn validate(&self) -> Result<(), ProjectionProtocolError> {
        if self.checkpoint_probes.len() > MAX_PROJECTION_QUERY_CHECKPOINT_PROBES {
            return Err(ProjectionProtocolError::InvalidBatch(format!(
                "projection query snapshot has {} checkpoint probes; maximum is {}",
                self.checkpoint_probes.len(),
                MAX_PROJECTION_QUERY_CHECKPOINT_PROBES
            )));
        }
        if self.scope.model() != self.schema.model_name {
            return Err(ProjectionProtocolError::ScopeMismatch {
                field: "projection query model",
            });
        }
        crate::table::validate_key(&self.schema, &self.key)?;

        let mut probes = std::collections::HashSet::new();
        for probe in &self.checkpoint_probes {
            if &probe.topology != self.scope.topology() {
                return Err(ProjectionProtocolError::ScopeMismatch {
                    field: "projection query checkpoint topology",
                });
            }
            if &probe.partition != self.scope.projection_partition() {
                return Err(ProjectionProtocolError::ScopeMismatch {
                    field: "projection query checkpoint partition",
                });
            }
            if !probes.insert((probe.source.clone(), probe.generation)) {
                return Err(ProjectionProtocolError::InvalidBatch(
                    "projection query snapshot repeats one source/generation checkpoint probe"
                        .into(),
                ));
            }
        }
        Ok(())
    }
}

/// Result for one explicit checkpoint probe in a query snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProjectionCheckpointSnapshot {
    pub(crate) probe: ProjectionCheckpointProbe,
    pub(crate) checkpoint: Option<ProjectionCheckpoint>,
}

/// Physical row and every causal fence needed to consume it safely.
///
/// All fields come from one adapter snapshot. In particular, `change_head` is
/// the live-resume boundary for this partition and must never be fetched after
/// the row/revision in a second independent read.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ProjectionQuerySnapshot {
    pub(crate) row: Option<RowValues>,
    pub(crate) record: Option<ProjectionRecordMetadata>,
    pub(crate) checkpoints: Vec<ProjectionCheckpointSnapshot>,
    pub(crate) change_head: Option<ProjectionChangeCursor>,
    pub(crate) compacted_through: u64,
}

/// Sanitized live boundary for one exact projector partition, read inside the
/// same adapter snapshot as a GraphQL query when used by Task 16.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProjectionPartitionSnapshot {
    pub(crate) head: Option<ProjectionChangeCursor>,
    pub(crate) compacted_through: u64,
}

/// A bounded set of row/evidence probes that must share one adapter snapshot.
///
/// This is the adapter-neutral convenience for known keys. Dynamic GraphQL
/// list/relationship plans use the SQL repeatable-snapshot session so their
/// physical membership query runs before these probes on the same connection.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ProjectionQuerySnapshotBatchRequest {
    pub(crate) requests: Vec<ProjectionQuerySnapshotRequest>,
}

impl ProjectionQuerySnapshotBatchRequest {
    pub(crate) fn new(
        requests: Vec<ProjectionQuerySnapshotRequest>,
    ) -> Result<Self, ProjectionProtocolError> {
        let request = Self { requests };
        request.validate()?;
        Ok(request)
    }

    pub(crate) fn validate(&self) -> Result<(), ProjectionProtocolError> {
        if self.requests.len() > MAX_PROJECTION_QUERY_BATCH_ROWS {
            return Err(ProjectionProtocolError::InvalidBatch(format!(
                "projection query snapshot batch has {} rows; maximum is {}",
                self.requests.len(),
                MAX_PROJECTION_QUERY_BATCH_ROWS
            )));
        }
        let checkpoint_probes = self.requests.iter().try_fold(0usize, |total, request| {
            total
                .checked_add(request.checkpoint_probes.len())
                .ok_or_else(|| {
                    ProjectionProtocolError::InvalidBatch(
                        "projection query snapshot batch checkpoint-probe count overflowed".into(),
                    )
                })
        })?;
        if checkpoint_probes > MAX_PROJECTION_QUERY_BATCH_CHECKPOINT_PROBES {
            return Err(ProjectionProtocolError::InvalidBatch(format!(
                "projection query snapshot batch has {checkpoint_probes} aggregate checkpoint probes; maximum is {}",
                MAX_PROJECTION_QUERY_BATCH_CHECKPOINT_PROBES
            )));
        }
        for request in &self.requests {
            request.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ProjectionQuerySnapshotBatch {
    pub(crate) snapshots: Vec<ProjectionQuerySnapshot>,
}

/// One exact command obligation evidence probe.
///
/// The scope is already compiler-bound and canonical. A store must compare all
/// of topology, partition, causation, model, observation kind, key bytes, and
/// key digest; a digest lookup alone is never evidence.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ProjectionObligationEvidenceRequest {
    pub(crate) causation_id: String,
    pub(crate) scope: ProjectionRecordScope,
    pub(crate) kind: ProjectionObservationKind,
}

impl ProjectionObligationEvidenceRequest {
    pub(crate) fn new(
        causation_id: impl Into<String>,
        scope: ProjectionRecordScope,
        kind: ProjectionObservationKind,
    ) -> Result<Self, ProjectionProtocolError> {
        let request = Self {
            causation_id: bounded_opaque(
                "projection causation ID",
                causation_id,
                MAX_CAUSATION_ID_BYTES,
            )?,
            scope,
            kind,
        };
        request.validate()?;
        Ok(request)
    }

    pub(crate) fn validate(&self) -> Result<(), ProjectionProtocolError> {
        bounded_opaque(
            "projection causation ID",
            self.causation_id.clone(),
            MAX_CAUSATION_ID_BYTES,
        )?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ProjectionObligationEvidence {
    Pending,
    Observed(ProjectionObservation),
    TerminalFailure(ProjectionFailure),
}

/// Bounded obligation probes evaluated from one adapter snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProjectionObligationEvidenceBatchRequest {
    pub(crate) requests: Vec<ProjectionObligationEvidenceRequest>,
}

impl ProjectionObligationEvidenceBatchRequest {
    pub(crate) fn new(
        requests: Vec<ProjectionObligationEvidenceRequest>,
    ) -> Result<Self, ProjectionProtocolError> {
        let request = Self { requests };
        request.validate()?;
        Ok(request)
    }

    pub(crate) fn validate(&self) -> Result<(), ProjectionProtocolError> {
        if self.requests.len() > MAX_PROJECTION_EVIDENCE_BATCH_ITEMS {
            return Err(ProjectionProtocolError::InvalidBatch(format!(
                "projection obligation evidence batch has {} probes; maximum is {}",
                self.requests.len(),
                MAX_PROJECTION_EVIDENCE_BATCH_ITEMS
            )));
        }
        let mut exact = std::collections::HashSet::new();
        for request in &self.requests {
            request.validate()?;
            if !exact.insert((request.causation_id.as_str(), &request.scope, request.kind)) {
                return Err(ProjectionProtocolError::InvalidBatch(
                    "projection obligation evidence batch repeats an exact probe".into(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ProjectionObligationEvidenceBatch {
    pub(crate) evidence: Vec<ProjectionObligationEvidence>,
}

/// A typed physical-row key whose causal partition is deliberately unknown.
///
/// The registered codec supplies the topology, schema, and canonical key. The
/// store may only return a live record after recovering its exact partition
/// from durable metadata and proving that identity is unique.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ProjectionLiveRecordRequest {
    pub(crate) schema: Arc<TableSchema>,
    pub(crate) topology: ProjectorTopologyId,
    pub(crate) key: RowKey,
    pub(crate) canonical_key_bytes: Vec<u8>,
    pub(crate) canonical_key_hash: [u8; 32],
}

impl ProjectionLiveRecordRequest {
    pub(crate) fn new(
        codec: &ProjectionScopeCodec,
        model: &str,
        key: RowKey,
    ) -> Result<Self, ProjectionProtocolError> {
        let schema = codec.registered_schema_owned(model).map_err(|error| {
            ProjectionProtocolError::InvalidBatch(format!(
                "invalid projection live-record model: {error}"
            ))
        })?;
        crate::table::validate_key(&schema, &key)?;
        let canonical_key_bytes =
            codec
                .encode_unpartitioned_row_key(model, &key)
                .map_err(|error| {
                    ProjectionProtocolError::InvalidBatch(format!(
                        "invalid projection live-record key: {error}"
                    ))
                })?;
        let canonical_key_hash = ProjectionRecordScope::key_digest_for(&canonical_key_bytes);
        Ok(Self {
            schema,
            topology: codec.topology().clone(),
            key,
            canonical_key_bytes,
            canonical_key_hash,
        })
    }

    pub(crate) fn model(&self) -> &str {
        &self.schema.model_name
    }

    pub(crate) fn validate(&self) -> Result<(), ProjectionProtocolError> {
        crate::table::validate_key(&self.schema, &self.key)?;
        if self.canonical_key_bytes.is_empty()
            || self.canonical_key_bytes.len() > super::MAX_PROJECTION_RECORD_KEY_BYTES
        {
            return Err(ProjectionProtocolError::InvalidBatch(
                "projection live-record canonical key is empty or oversized".into(),
            ));
        }
        if ProjectionRecordScope::key_digest_for(&self.canonical_key_bytes)
            != self.canonical_key_hash
        {
            return Err(ProjectionProtocolError::InvalidBatch(
                "projection live-record canonical key bytes and digest disagree".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ProjectionLiveRecordBatchRequest {
    pub(crate) requests: Vec<ProjectionLiveRecordRequest>,
}

impl ProjectionLiveRecordBatchRequest {
    pub(crate) fn new(
        requests: Vec<ProjectionLiveRecordRequest>,
    ) -> Result<Self, ProjectionProtocolError> {
        let request = Self { requests };
        request.validate()?;
        Ok(request)
    }

    pub(crate) fn validate(&self) -> Result<(), ProjectionProtocolError> {
        if self.requests.len() > MAX_PROJECTION_EVIDENCE_BATCH_ITEMS {
            return Err(ProjectionProtocolError::InvalidBatch(format!(
                "projection live-record batch has {} rows; maximum is {}",
                self.requests.len(),
                MAX_PROJECTION_EVIDENCE_BATCH_ITEMS
            )));
        }
        let mut identities = std::collections::HashSet::new();
        for request in &self.requests {
            request.validate()?;
            if !identities.insert((
                &request.topology,
                request.model(),
                request.canonical_key_hash,
            )) {
                return Err(ProjectionProtocolError::InvalidBatch(
                    "projection live-record batch repeats a topology/model/key identity".into(),
                ));
            }
        }
        Ok(())
    }
}

/// Results are aligned positionally with the validated request batch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ProjectionLiveRecordBatch {
    pub(crate) records: Vec<Option<ProjectionRecordMetadata>>,
}

/// Exact immutable identity that a repaired partition must retry first.
///
/// `failed_generation` identifies the generation that stopped. The enclosing
/// runtime state carries the newly active generation under which this exact
/// input must be retried.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProjectionPendingRetry {
    pub(crate) failure_id: String,
    pub(crate) input: ProjectionInputCursor,
    pub(crate) input_fingerprint: ProjectionInputFingerprint,
    pub(crate) message_id: String,
    pub(crate) causation_id: String,
    pub(crate) failed_generation: ProjectionGeneration,
    pub(crate) gap_free: bool,
}

/// Runtime-visible fences for one exact projector partition.
///
/// A missing value means the partition has never been bootstrapped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProjectionPartitionRuntimeState {
    pub(crate) active_generation: ProjectionGeneration,
    pub(crate) stopped_failure_id: Option<String>,
    pub(crate) pending_retry: Option<ProjectionPendingRetry>,
}

/// Exact durable observation of one command causation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectionObservation {
    pub causation_id: String,
    pub kind: ProjectionObservationKind,
    /// Present for record observations; absent for embedded/dependency scopes.
    pub revision: Option<RecordRevision>,
    /// Canonical dependency scope when no record revision exists.
    pub scope: ProjectionRecordScope,
    pub change: ProjectionChangeCursor,
}

/// Durable terminal failure for one exact input and repair generation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectionFailure {
    pub failure_id: String,
    pub input: ProjectionInputCursor,
    pub input_fingerprint: ProjectionInputFingerprint,
    pub message_id: String,
    pub causation_id: String,
    pub generation: ProjectionGeneration,
    /// Whether the failed input source promised gap-free ordered delivery.
    pub gap_free: bool,
    pub failure_code: String,
    pub failure_bytes: Vec<u8>,
    pub failure_digest: [u8; 32],
    pub change: ProjectionChangeCursor,
}

/// Adapter-resolved exact scope for one globally unique projection failure ID.
///
/// This is crate-private so an operator supplies only the opaque failure handle;
/// the durable store, not the caller, reconstructs canonical topology and
/// partition authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProjectionFailureLocation {
    pub(crate) topology: ProjectorTopologyId,
    pub(crate) partition: ProjectionPartition,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProjectionChangeKind {
    /// A successful input with no row/observation change. This makes the exact
    /// checkpoint resumable without pretending an old change head was newly
    /// allocated and allows gap-free barriers to advance.
    Checkpoint,
    RecordUpsert,
    RecordDelete,
    RecordRecreate,
    Observation,
    Failure,
}

impl ProjectionChangeKind {
    pub(crate) fn as_storage_str(self) -> &'static str {
        match self {
            Self::Checkpoint => "checkpoint",
            Self::RecordUpsert => "record_upsert",
            Self::RecordDelete => "record_delete",
            Self::RecordRecreate => "record_recreate",
            Self::Observation => "observation",
            Self::Failure => "failure",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectionChange {
    pub cursor: ProjectionChangeCursor,
    pub kind: ProjectionChangeKind,
    pub causation_id: String,
    pub observation_kind: Option<ProjectionObservationKind>,
    /// Canonical record/dependency identity for row and observation changes.
    /// Checkpoint-only and failure entries deliberately carry no model scope.
    pub scope: Option<ProjectionRecordScope>,
    pub revision: Option<RecordRevision>,
    pub failure_id: Option<String>,
}

/// Result and exact evidence produced by an asynchronous projection commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectionCommitResult {
    pub outcome: ProjectionCommitOutcome,
    pub checkpoint: Option<ProjectionCheckpoint>,
    pub records: Vec<ProjectionRecordMetadata>,
    pub changes: Vec<ProjectionChange>,
}

impl ProjectionCommitResult {
    pub(crate) fn not_applied(
        outcome: ProjectionCommitOutcome,
        checkpoint: Option<ProjectionCheckpoint>,
    ) -> Self {
        debug_assert!(outcome != ProjectionCommitOutcome::Applied);
        Self {
            outcome,
            checkpoint,
            records: Vec::new(),
            changes: Vec::new(),
        }
    }
}

/// Exact evidence allocated for a same-transaction projected command.
///
/// The command ledger stores the canonical replay form produced by
/// [`replay_value`](Self::replay_value), so response-loss recovery returns the
/// revision and change allocated by the original transaction rather than the
/// record's later head.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SameTransactionProjectionEvidence {
    pub(crate) records: Vec<ProjectionRecordMetadata>,
    pub(crate) changes: Vec<ProjectionChange>,
    pub(crate) observations: Vec<ProjectionObservation>,
}

impl SameTransactionProjectionEvidence {
    pub(crate) fn replay_value(&self) -> serde_json::Value {
        serde_json::to_value(SameTransactionReplayEnvelope::from(self))
            .expect("same-transaction projection evidence contains only serializable primitives")
    }

    pub(crate) fn from_replay_value(value: &serde_json::Value) -> Result<Self, String> {
        let decoded: SameTransactionReplayEnvelope = serde_json::from_value(value.clone())
            .map_err(|error| format!("direct projection evidence is invalid: {error}"))?;
        let evidence = decoded.into_evidence()?;
        let canonical = serde_json::to_value(SameTransactionReplayEnvelope::from(&evidence))
            .map_err(|error| format!("direct projection evidence cannot be normalized: {error}"))?;
        if canonical != *value {
            return Err(
                "direct projection evidence contains unknown or non-canonical fields".into(),
            );
        }
        Ok(evidence)
    }

    pub(crate) fn validate_replay_value(value: &serde_json::Value) -> Result<(), String> {
        Self::from_replay_value(value).map(|_| ())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SameTransactionReplayEnvelope {
    version: u16,
    records: Vec<ReplayRecord>,
    changes: Vec<ReplayChange>,
    observations: Vec<ReplayObservation>,
}

impl From<&SameTransactionProjectionEvidence> for SameTransactionReplayEnvelope {
    fn from(evidence: &SameTransactionProjectionEvidence) -> Self {
        Self {
            version: 1,
            records: evidence.records.iter().map(ReplayRecord::from).collect(),
            changes: evidence.changes.iter().map(ReplayChange::from).collect(),
            observations: evidence
                .observations
                .iter()
                .map(ReplayObservation::from)
                .collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayScope {
    topology_version: u32,
    topology_name: String,
    topology_digest: String,
    partition: String,
    partition_digest: String,
    model: String,
    key: String,
    key_digest: String,
}

impl From<&ProjectionRecordScope> for ReplayScope {
    fn from(scope: &ProjectionRecordScope) -> Self {
        Self {
            topology_version: scope.topology().version(),
            topology_name: scope.topology().name().to_string(),
            topology_digest: digest_hex(&scope.topology().digest()),
            partition: bytes_hex(scope.projection_partition().canonical_bytes()),
            partition_digest: digest_hex(&scope.projection_partition().digest()),
            model: scope.model().to_string(),
            key: bytes_hex(scope.canonical_key_bytes()),
            key_digest: digest_hex(&scope.key_digest()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayRevision {
    scope: ReplayScope,
    incarnation: u64,
    revision: u64,
}

impl From<&RecordRevision> for ReplayRevision {
    fn from(revision: &RecordRevision) -> Self {
        Self {
            scope: ReplayScope::from(revision.scope()),
            incarnation: revision.incarnation(),
            revision: revision.revision(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayCursor {
    topology_version: u32,
    topology_name: String,
    topology_digest: String,
    partition: String,
    partition_digest: String,
    epoch: String,
    position: u64,
}

impl From<&ProjectionChangeCursor> for ReplayCursor {
    fn from(cursor: &ProjectionChangeCursor) -> Self {
        Self {
            topology_version: cursor.topology().version(),
            topology_name: cursor.topology().name().to_string(),
            topology_digest: digest_hex(&cursor.topology().digest()),
            partition: bytes_hex(cursor.projection_partition().canonical_bytes()),
            partition_digest: digest_hex(&cursor.projection_partition().digest()),
            epoch: cursor.epoch().as_str().to_string(),
            position: cursor.position(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayRecord {
    revision: ReplayRevision,
    tombstone: bool,
    change: ReplayCursor,
}

impl From<&ProjectionRecordMetadata> for ReplayRecord {
    fn from(record: &ProjectionRecordMetadata) -> Self {
        Self {
            revision: ReplayRevision::from(&record.revision),
            tombstone: record.tombstone,
            change: ReplayCursor::from(&record.change),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayChange {
    cursor: ReplayCursor,
    kind: String,
    causation_id: String,
    observation_kind: Option<String>,
    scope: Option<ReplayScope>,
    revision: Option<ReplayRevision>,
    failure_id: Option<String>,
}

impl From<&ProjectionChange> for ReplayChange {
    fn from(change: &ProjectionChange) -> Self {
        Self {
            cursor: ReplayCursor::from(&change.cursor),
            kind: change.kind.as_storage_str().to_string(),
            causation_id: change.causation_id.clone(),
            observation_kind: change
                .observation_kind
                .map(|kind| kind.as_storage_str().to_string()),
            scope: change.scope.as_ref().map(ReplayScope::from),
            revision: change.revision.as_ref().map(ReplayRevision::from),
            failure_id: change.failure_id.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayObservation {
    causation_id: String,
    kind: String,
    revision: Option<ReplayRevision>,
    scope: ReplayScope,
    change: ReplayCursor,
}

impl From<&ProjectionObservation> for ReplayObservation {
    fn from(observation: &ProjectionObservation) -> Self {
        Self {
            causation_id: observation.causation_id.clone(),
            kind: observation.kind.as_storage_str().to_string(),
            revision: observation.revision.as_ref().map(ReplayRevision::from),
            scope: ReplayScope::from(&observation.scope),
            change: ReplayCursor::from(&observation.change),
        }
    }
}

impl SameTransactionReplayEnvelope {
    fn into_evidence(self) -> Result<SameTransactionProjectionEvidence, String> {
        if self.version != 1 {
            return Err(format!(
                "direct projection evidence version {} is unsupported",
                self.version
            ));
        }
        let evidence = SameTransactionProjectionEvidence {
            records: self
                .records
                .into_iter()
                .map(ReplayRecord::into_record)
                .collect::<Result<_, _>>()?,
            changes: self
                .changes
                .into_iter()
                .map(ReplayChange::into_change)
                .collect::<Result<_, _>>()?,
            observations: self
                .observations
                .into_iter()
                .map(ReplayObservation::into_observation)
                .collect::<Result<_, _>>()?,
        };
        validate_same_transaction_replay_evidence(&evidence)?;
        Ok(evidence)
    }
}

impl ReplayScope {
    fn into_scope(self) -> Result<ProjectionRecordScope, String> {
        let topology = decode_replay_topology(
            self.topology_version,
            self.topology_name,
            self.topology_digest,
        )?;
        let partition = decode_replay_partition(self.partition, self.partition_digest)?;
        let key = decode_replay_hex("record key", &self.key, MAX_PROJECTION_RECORD_KEY_BYTES)?;
        let scope = ProjectionRecordScope::new(topology, partition, self.model, key)
            .map_err(|error| replay_validation_error("record scope", error))?;
        if digest_hex(&scope.key_digest()) != self.key_digest {
            return Err(
                "direct projection evidence record key digest does not match its canonical bytes"
                    .into(),
            );
        }
        Ok(scope)
    }
}

impl ReplayRevision {
    fn into_revision(self) -> Result<RecordRevision, String> {
        RecordRevision::new(self.scope.into_scope()?, self.incarnation, self.revision)
            .map_err(|error| replay_validation_error("record revision", error))
    }
}

impl ReplayCursor {
    fn into_cursor(self) -> Result<ProjectionChangeCursor, String> {
        ProjectionChangeCursor::new(
            decode_replay_topology(
                self.topology_version,
                self.topology_name,
                self.topology_digest,
            )?,
            decode_replay_partition(self.partition, self.partition_digest)?,
            ProjectionEpoch::new(self.epoch)
                .map_err(|error| replay_validation_error("change epoch", error))?,
            self.position,
        )
        .map_err(|error| replay_validation_error("change cursor", error))
    }
}

impl ReplayRecord {
    fn into_record(self) -> Result<ProjectionRecordMetadata, String> {
        Ok(ProjectionRecordMetadata {
            revision: self.revision.into_revision()?,
            tombstone: self.tombstone,
            change: self.change.into_cursor()?,
        })
    }
}

impl ReplayChange {
    fn into_change(self) -> Result<ProjectionChange, String> {
        Ok(ProjectionChange {
            cursor: self.cursor.into_cursor()?,
            kind: decode_replay_change_kind(&self.kind)?,
            causation_id: bounded_opaque(
                "projection causation ID",
                self.causation_id,
                MAX_CAUSATION_ID_BYTES,
            )
            .map_err(|error| replay_validation_error("change causation", error))?,
            observation_kind: self
                .observation_kind
                .as_deref()
                .map(decode_replay_observation_kind)
                .transpose()?,
            scope: self.scope.map(ReplayScope::into_scope).transpose()?,
            revision: self
                .revision
                .map(ReplayRevision::into_revision)
                .transpose()?,
            failure_id: self
                .failure_id
                .map(|failure_id| {
                    bounded_opaque("projection failure ID", failure_id, MAX_FAILURE_ID_BYTES)
                        .map_err(|error| replay_validation_error("change failure ID", error))
                })
                .transpose()?,
        })
    }
}

impl ReplayObservation {
    fn into_observation(self) -> Result<ProjectionObservation, String> {
        Ok(ProjectionObservation {
            causation_id: bounded_opaque(
                "projection causation ID",
                self.causation_id,
                MAX_CAUSATION_ID_BYTES,
            )
            .map_err(|error| replay_validation_error("observation causation", error))?,
            kind: decode_replay_observation_kind(&self.kind)?,
            revision: self
                .revision
                .map(ReplayRevision::into_revision)
                .transpose()?,
            scope: self.scope.into_scope()?,
            change: self.change.into_cursor()?,
        })
    }
}

fn decode_replay_topology(
    version: u32,
    name: String,
    digest: String,
) -> Result<ProjectorTopologyId, String> {
    ProjectorTopologyId::new(
        version,
        name,
        decode_replay_digest("topology digest", &digest)?,
    )
    .map_err(|error| replay_validation_error("topology", error))
}

fn decode_replay_partition(
    canonical: String,
    digest: String,
) -> Result<ProjectionPartition, String> {
    let partition = ProjectionPartition::new(decode_replay_hex(
        "partition",
        &canonical,
        MAX_PROJECTION_PARTITION_BYTES,
    )?)
    .map_err(|error| replay_validation_error("partition", error))?;
    if digest_hex(&partition.digest()) != digest {
        return Err(
            "direct projection evidence partition digest does not match its canonical bytes".into(),
        );
    }
    Ok(partition)
}

fn decode_replay_digest(field: &str, value: &str) -> Result<[u8; 32], String> {
    decode_replay_hex(field, value, 32)?
        .try_into()
        .map_err(|_| format!("direct projection evidence {field} must contain exactly 32 bytes"))
}

fn decode_replay_hex(field: &str, value: &str, max_bytes: usize) -> Result<Vec<u8>, String> {
    if value.len() % 2 != 0 || value.len() > max_bytes.saturating_mul(2) {
        return Err(format!(
            "direct projection evidence {field} is not bounded canonical hexadecimal"
        ));
    }
    let mut decoded = Vec::with_capacity(value.len() / 2);
    for pair in value.as_bytes().chunks_exact(2) {
        let high = decode_replay_hex_nibble(pair[0]).ok_or_else(|| {
            format!("direct projection evidence {field} is not lowercase hexadecimal")
        })?;
        let low = decode_replay_hex_nibble(pair[1]).ok_or_else(|| {
            format!("direct projection evidence {field} is not lowercase hexadecimal")
        })?;
        decoded.push((high << 4) | low);
    }
    Ok(decoded)
}

fn decode_replay_hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

fn decode_replay_change_kind(value: &str) -> Result<ProjectionChangeKind, String> {
    match value {
        "checkpoint" => Ok(ProjectionChangeKind::Checkpoint),
        "record_upsert" => Ok(ProjectionChangeKind::RecordUpsert),
        "record_delete" => Ok(ProjectionChangeKind::RecordDelete),
        "record_recreate" => Ok(ProjectionChangeKind::RecordRecreate),
        "observation" => Ok(ProjectionChangeKind::Observation),
        "failure" => Ok(ProjectionChangeKind::Failure),
        _ => Err(format!(
            "direct projection evidence has unknown change kind `{value}`"
        )),
    }
}

fn decode_replay_observation_kind(value: &str) -> Result<ProjectionObservationKind, String> {
    match value {
        "record" => Ok(ProjectionObservationKind::Record),
        "dependency" => Ok(ProjectionObservationKind::Dependency),
        _ => Err(format!(
            "direct projection evidence has unknown observation kind `{value}`"
        )),
    }
}

fn validate_same_transaction_replay_evidence(
    evidence: &SameTransactionProjectionEvidence,
) -> Result<(), String> {
    let [record] = evidence.records.as_slice() else {
        return Err("direct projection evidence must contain exactly one projected record".into());
    };
    let [change] = evidence.changes.as_slice() else {
        return Err("direct projection evidence must contain exactly one record change".into());
    };
    let [observation] = evidence.observations.as_slice() else {
        return Err(
            "direct projection evidence must contain exactly one record observation".into(),
        );
    };
    if record.tombstone {
        return Err("direct projection evidence cannot replay a tombstone".into());
    }
    if change.kind != ProjectionChangeKind::RecordUpsert
        || change.observation_kind.is_some()
        || change.failure_id.is_some()
    {
        return Err("direct projection evidence change must be a plain record upsert".into());
    }
    if change.scope.as_ref() != Some(record.revision.scope())
        || change.revision.as_ref() != Some(&record.revision)
    {
        return Err(
            "direct projection evidence change does not match the projected record revision".into(),
        );
    }
    if change.cursor.topology() != record.revision.scope().topology()
        || change.cursor.projection_partition() != record.revision.scope().projection_partition()
    {
        return Err(
            "direct projection evidence change cursor does not match its record scope".into(),
        );
    }
    if record.change != change.cursor {
        return Err("direct projection evidence record and change cursors do not match".into());
    }
    if observation.kind != ProjectionObservationKind::Record
        || observation.scope != *record.revision.scope()
        || observation.revision.as_ref() != Some(&record.revision)
    {
        return Err(
            "direct projection evidence observation does not match the projected record revision"
                .into(),
        );
    }
    if observation.change != change.cursor || observation.causation_id != change.causation_id {
        return Err(
            "direct projection evidence observation does not match its record change".into(),
        );
    }
    Ok(())
}

fn replay_validation_error(field: &str, error: impl fmt::Display) -> String {
    format!("direct projection evidence {field} is invalid: {error}")
}

fn bytes_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectionChangeRead {
    Changes {
        head: Option<ProjectionChangeCursor>,
        compacted_through: u64,
        changes: Vec<ProjectionChange>,
    },
    ResetRequired {
        head: Option<ProjectionChangeCursor>,
        compacted_through: u64,
    },
}

/// Adapter contract for atomic causal projection persistence.
pub(crate) trait ProjectionProtocolStore: Send + Sync {
    /// Install model-wide causal ownership before projector traffic begins.
    /// This bootstrap marker closes the absent-row race with legacy/raw write
    /// plans; per-partition ownership is still verified inside each commit.
    fn register_projection_models<'a>(
        &'a self,
        topology: &'a ProjectorTopologyId,
        ownership: &'a [ProjectionModelOwnership],
    ) -> impl Future<Output = Result<(), ProjectionProtocolError>> + Send + 'a;

    fn commit_projection(
        &self,
        batch: ProjectionCommitBatch,
    ) -> impl Future<Output = Result<ProjectionCommitResult, ProjectionProtocolError>> + Send + '_;

    fn record_projection_failure(
        &self,
        batch: ProjectionFailureBatch,
    ) -> impl Future<Output = Result<ProjectionFailure, ProjectionProtocolError>> + Send + '_;

    fn projection_checkpoint<'a>(
        &'a self,
        cursor_scope: &'a ProjectionInputCursor,
        generation: ProjectionGeneration,
    ) -> impl Future<Output = Result<Option<ProjectionCheckpoint>, ProjectionProtocolError>> + Send + 'a;

    fn projection_record<'a>(
        &'a self,
        scope: &'a ProjectionRecordScope,
    ) -> impl Future<Output = Result<Option<ProjectionRecordMetadata>, ProjectionProtocolError>>
           + Send
           + 'a;

    fn projection_input_disposition<'a>(
        &'a self,
        input: &'a TrustedProjectionInput,
    ) -> impl Future<Output = Result<ProjectionInputDisposition, ProjectionProtocolError>> + Send + 'a;

    /// Read one physical row, its record metadata, requested source
    /// checkpoints, and the partition live-resume boundary from one atomic
    /// adapter snapshot.
    fn projection_query_snapshot<'a>(
        &'a self,
        request: &'a ProjectionQuerySnapshotRequest,
    ) -> impl Future<Output = Result<ProjectionQuerySnapshot, ProjectionProtocolError>> + Send + 'a;

    fn projection_query_snapshot_batch<'a>(
        &'a self,
        request: &'a ProjectionQuerySnapshotBatchRequest,
    ) -> impl Future<Output = Result<ProjectionQuerySnapshotBatch, ProjectionProtocolError>> + Send + 'a;

    fn projection_obligation_evidence_batch<'a>(
        &'a self,
        request: &'a ProjectionObligationEvidenceBatchRequest,
    ) -> impl Future<Output = Result<ProjectionObligationEvidenceBatch, ProjectionProtocolError>>
           + Send
           + 'a;

    fn projection_live_record_batch<'a>(
        &'a self,
        request: &'a ProjectionLiveRecordBatchRequest,
    ) -> impl Future<Output = Result<ProjectionLiveRecordBatch, ProjectionProtocolError>> + Send + 'a;

    fn projection_partition_runtime_state<'a>(
        &'a self,
        topology: &'a ProjectorTopologyId,
        partition: &'a ProjectionPartition,
    ) -> impl Future<Output = Result<Option<ProjectionPartitionRuntimeState>, ProjectionProtocolError>>
           + Send
           + 'a;

    fn projection_observation<'a>(
        &'a self,
        causation_id: &'a str,
        scope: &'a ProjectionRecordScope,
        kind: ProjectionObservationKind,
    ) -> impl Future<Output = Result<Option<ProjectionObservation>, ProjectionProtocolError>> + Send + 'a;

    fn projection_changes<'a>(
        &'a self,
        topology: &'a ProjectorTopologyId,
        partition: &'a ProjectionPartition,
        after: Option<&'a ProjectionChangeCursor>,
        limit: usize,
    ) -> impl Future<Output = Result<ProjectionChangeRead, ProjectionProtocolError>> + Send + 'a;

    /// Start an explicitly linked repair generation for the immutable failure
    /// currently stopping this exact partition. Implementations copy every
    /// last-good source checkpoint, atomically switch the active generation,
    /// and only then clear the stop fence.
    fn repair_projection<'a>(
        &'a self,
        topology: &'a ProjectorTopologyId,
        partition: &'a ProjectionPartition,
        failure_id: &'a str,
    ) -> impl Future<Output = Result<ProjectionGeneration, ProjectionProtocolError>> + Send + 'a;

    /// Compact durable changes through the supplied exact cursor. The returned
    /// watermark is the last removed position; adapters never advertise a
    /// larger window than they actually retain.
    fn compact_projection_changes<'a>(
        &'a self,
        through: &'a ProjectionChangeCursor,
    ) -> impl Future<Output = Result<u64, ProjectionProtocolError>> + Send + 'a;

    fn projection_failure<'a>(
        &'a self,
        topology: &'a ProjectorTopologyId,
        partition: &'a ProjectionPartition,
        failure_id: &'a str,
    ) -> impl Future<Output = Result<Option<ProjectionFailure>, ProjectionProtocolError>> + Send + 'a;

    /// Resolve a globally unique durable failure ID to its exact stored scope.
    ///
    /// Adapters must reconstruct and validate the canonical topology/partition
    /// bytes they own. This is the safe basis for the public opaque repair
    /// handle; callers never provide tenant-bearing partition bytes.
    fn projection_failure_location<'a>(
        &'a self,
        failure_id: &'a str,
    ) -> impl Future<Output = Result<Option<ProjectionFailureLocation>, ProjectionProtocolError>>
           + Send
           + 'a;
}

#[derive(Debug)]
#[non_exhaustive]
pub enum ProjectionProtocolError {
    Validation(ProjectionProtocolValidationError),
    Repository(RepositoryError),
    Table(TableStoreError),
    InvalidBatch(String),
    ScopeMismatch {
        field: &'static str,
    },
    IncomparableInput,
    InputCorruption,
    MessageIdReuse {
        message_id: String,
    },
    GenerationFenced {
        expected: u64,
        actual: u64,
    },
    PartitionStopped {
        failure_id: String,
    },
    RecordMissing {
        model: String,
    },
    RecordAlreadyExists {
        model: String,
    },
    RecordRevisionConflict {
        model: String,
        expected_incarnation: u64,
        expected_revision: u64,
        actual_incarnation: u64,
        actual_revision: u64,
    },
    RecordTombstoned {
        model: String,
    },
    RecreateRequiresTombstone {
        model: String,
    },
    CausalWriteRequired {
        table: String,
    },
    PositionOverflow {
        domain: &'static str,
    },
}

impl fmt::Display for ProjectionProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Validation(error) => write!(formatter, "invalid projection protocol value: {error}"),
            Self::Repository(error) => write!(formatter, "projection repository error: {error}"),
            Self::Table(error) => write!(formatter, "projection table error: {error}"),
            Self::InvalidBatch(message) => write!(formatter, "invalid projection batch: {message}"),
            Self::ScopeMismatch { field } => write!(formatter, "{field} does not match the projection input scope"),
            Self::IncomparableInput => formatter.write_str("projection input is incomparable with the durable checkpoint"),
            Self::InputCorruption => formatter.write_str("the same projection input cursor was reused with different content"),
            Self::MessageIdReuse { message_id } => write!(formatter, "projection message ID `{message_id}` was reused for a different input"),
            Self::GenerationFenced { expected, actual } => write!(formatter, "projection generation {actual} is fenced; active generation is {expected}"),
            Self::PartitionStopped { failure_id } => write!(formatter, "projection partition is stopped by terminal failure `{failure_id}`"),
            Self::RecordMissing { model } => write!(formatter, "projection record `{model}` does not exist"),
            Self::RecordAlreadyExists { model } => write!(formatter, "projection record `{model}` already exists"),
            Self::RecordRevisionConflict { model, expected_incarnation, expected_revision, actual_incarnation, actual_revision } => write!(formatter, "projection record `{model}` expected revision ({expected_incarnation}, {expected_revision}) but found ({actual_incarnation}, {actual_revision})"),
            Self::RecordTombstoned { model } => write!(formatter, "projection record `{model}` is tombstoned; use explicit recreate"),
            Self::RecreateRequiresTombstone { model } => write!(formatter, "projection record `{model}` can only be recreated from its exact tombstone revision"),
            Self::CausalWriteRequired { table } => write!(formatter, "table `{table}` is causal-owned and requires the projection commit path"),
            Self::PositionOverflow { domain } => write!(formatter, "{domain} position overflow"),
        }
    }
}

impl std::error::Error for ProjectionProtocolError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Validation(error) => Some(error),
            Self::Repository(error) => Some(error),
            Self::Table(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ProjectionProtocolValidationError> for ProjectionProtocolError {
    fn from(error: ProjectionProtocolValidationError) -> Self {
        Self::Validation(error)
    }
}

impl From<RepositoryError> for ProjectionProtocolError {
    fn from(error: RepositoryError) -> Self {
        Self::Repository(error)
    }
}

impl From<TableStoreError> for ProjectionProtocolError {
    fn from(error: TableStoreError) -> Self {
        Self::Table(error)
    }
}

fn validate_scope(
    topology: &ProjectorTopologyId,
    partition: &ProjectionPartition,
    scope: &ProjectionRecordScope,
) -> Result<(), ProjectionProtocolError> {
    if scope.topology() != topology {
        return Err(ProjectionProtocolError::ScopeMismatch {
            field: "projection record topology",
        });
    }
    if scope.projection_partition() != partition {
        return Err(ProjectionProtocolError::ScopeMismatch {
            field: "projection record partition",
        });
    }
    Ok(())
}

fn bounded_name(
    field: &'static str,
    value: impl Into<String>,
    max: usize,
) -> Result<String, ProjectionProtocolError> {
    let value = value.into();
    if value.is_empty()
        || value.len() > max
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(ProjectionProtocolError::InvalidBatch(format!(
            "{field} must contain 1..={max} non-whitespace, non-control UTF-8 bytes"
        )));
    }
    Ok(value)
}

fn bounded_opaque(
    field: &'static str,
    value: impl Into<String>,
    max: usize,
) -> Result<String, ProjectionProtocolError> {
    let value = value.into();
    if value.is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(ProjectionProtocolError::InvalidBatch(format!(
            "{field} must contain 1..={max} non-control UTF-8 bytes"
        )));
    }
    Ok(value)
}

fn domain_separated_digest(domain: &[u8], bytes: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(domain);
    digest.update((bytes.len() as u64).to_be_bytes());
    digest.update(bytes);
    digest.finalize().into()
}

fn digest_hex(bytes: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(64);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection_protocol::ResolvedProjectionObligation;
    use crate::table::{
        DeleteTableRowMutation, ExpectedVersion, PrimaryKey, RowKey, RowValue, TableSchema,
    };

    fn topology() -> ProjectorTopologyId {
        ProjectorTopologyId::new(1, "todos", [7; 32]).unwrap()
    }

    fn partition() -> ProjectionPartition {
        ProjectionPartition::new(b"tenant:a".to_vec()).unwrap()
    }

    fn scope(model: &str, key: &[u8]) -> ProjectionRecordScope {
        ProjectionRecordScope::new(topology(), partition(), model, key.to_vec()).unwrap()
    }

    fn schema() -> &'static TableSchema {
        Box::leak(Box::new(TableSchema {
            model_name: "TodoView".into(),
            table_name: "todo_views".into(),
            columns: Vec::new(),
            primary_key: PrimaryKey::new(["id"]),
            version_column: None,
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships: Vec::new(),
            kind: Default::default(),
        }))
    }

    fn same_transaction_evidence() -> SameTransactionProjectionEvidence {
        let scope = scope("TodoView", b"todo-1");
        let revision = RecordRevision::new(scope.clone(), 1, 1).unwrap();
        let change = ProjectionChangeCursor::new(
            topology(),
            partition(),
            ProjectionEpoch::new("changes-v1").unwrap(),
            1,
        )
        .unwrap();
        SameTransactionProjectionEvidence {
            records: vec![ProjectionRecordMetadata {
                revision: revision.clone(),
                tombstone: false,
                change: change.clone(),
            }],
            changes: vec![ProjectionChange {
                cursor: change.clone(),
                kind: ProjectionChangeKind::RecordUpsert,
                causation_id: "cause-1".into(),
                observation_kind: None,
                scope: Some(scope.clone()),
                revision: Some(revision.clone()),
                failure_id: None,
            }],
            observations: vec![ProjectionObservation {
                causation_id: "cause-1".into(),
                kind: ProjectionObservationKind::Record,
                revision: Some(revision),
                scope,
                change,
            }],
        }
    }

    fn failure_batch() -> ProjectionFailureBatch {
        let input = TrustedProjectionInput::mint(
            ProjectionInputCursor::new(
                topology(),
                partition(),
                ProjectionSource::new("todo_stream", b"todo-1".to_vec()).unwrap(),
                ProjectionEpoch::new("source-v1").unwrap(),
                1,
            )
            .unwrap(),
            ProjectionInputFingerprint::from_canonical_bytes(b"input-1"),
            "message-1",
            "cause-1",
            ProjectionGeneration::initial(),
            true,
        )
        .unwrap();
        ProjectionFailureBatch::new(
            input,
            ProjectionEpoch::new("changes-v1").unwrap(),
            "failure-1",
            "decode_error",
            b"bad payload".to_vec(),
        )
        .unwrap()
    }

    #[test]
    fn change_retention_is_nonzero_portable_and_bounded_by_default() {
        assert_eq!(
            ProjectionChangeRetention::default().max_retained_changes(),
            DEFAULT_MAX_RETAINED_PROJECTION_CHANGES
        );
        assert_eq!(
            ProjectionChangeRetention::new(0),
            Err(ProjectionProtocolValidationError::Zero {
                field: "projection retained change count",
            })
        );
        assert_eq!(
            ProjectionChangeRetention::new(u64::MAX),
            Err(ProjectionProtocolValidationError::TooLarge {
                field: "projection retained change count",
                value: u64::MAX,
                max: MAX_PROJECTION_POSITION,
            })
        );
        assert_eq!(
            ProjectionChangeRetention::new(MAX_PROJECTION_POSITION)
                .unwrap()
                .max_retained_changes(),
            MAX_PROJECTION_POSITION
        );
    }

    #[test]
    fn generations_are_nonzero_and_checked() {
        assert!(ProjectionGeneration::new(0).is_err());
        assert_eq!(ProjectionGeneration::initial().get(), 1);
        assert_eq!(
            ProjectionGeneration::new(7)
                .unwrap()
                .checked_next()
                .unwrap()
                .get(),
            8
        );
        assert_eq!(
            ProjectionGeneration::new(u64::MAX),
            Err(ProjectionProtocolValidationError::TooLarge {
                field: "projection generation",
                value: u64::MAX,
                max: MAX_PROJECTION_POSITION,
            })
        );
        assert!(matches!(
            ProjectionGeneration::new(MAX_PROJECTION_POSITION)
                .unwrap()
                .checked_next(),
            Err(ProjectionProtocolError::PositionOverflow {
                domain: "projection generation"
            })
        ));
    }

    #[test]
    fn trusted_input_identity_is_bounded_and_deterministic() {
        let cursor = ProjectionInputCursor::new(
            topology(),
            partition(),
            super::super::ProjectionSource::new("aggregate", b"todo:1".to_vec()).unwrap(),
            ProjectionEpoch::new("source-v1").unwrap(),
            0,
        )
        .unwrap();
        let left = ProjectionInputFingerprint::from_canonical_bytes(b"same");
        let right = ProjectionInputFingerprint::from_canonical_bytes(b"same");
        assert_eq!(left, right);
        let input = TrustedProjectionInput::mint(
            cursor,
            left,
            "message-1",
            "cause-1",
            ProjectionGeneration::initial(),
            false,
        )
        .unwrap();
        assert!(input.inbox_receipt().validate().is_ok());
        assert!(input.consumer_name().starts_with("projection:v1:"));
    }

    #[test]
    fn record_expectations_and_mutation_kinds_fail_closed() {
        let first_scope = scope("TodoView", b"1");
        let other_scope = scope("TodoView", b"2");
        let revision = RecordRevision::new(other_scope, 1, 1).unwrap();
        let delete = TableMutation::DeleteRow(DeleteTableRowMutation {
            schema: schema(),
            key: RowKey::new([("id", RowValue::String("1".into()))]),
            expected_version: ExpectedVersion::Any,
        });
        assert!(matches!(
            ProjectionRecordMutation::new(
                first_scope.clone(),
                delete.clone(),
                ProjectionRecordExpectation::Exact(revision),
                ProjectionMutationKind::Delete,
            ),
            Err(ProjectionProtocolError::ScopeMismatch { .. })
        ));
        assert!(matches!(
            ProjectionRecordMutation::new(
                first_scope,
                delete,
                ProjectionRecordExpectation::Missing,
                ProjectionMutationKind::Delete,
            ),
            Err(ProjectionProtocolError::InvalidBatch(_))
        ));
    }

    #[test]
    fn absent_and_explicit_null_obligation_partitions_survive_round_trip() {
        let topology = ProjectorTopologyId::new(1, "todos", [9; 32]).unwrap();
        let partition = super::super::ProjectionPartition::new(b"unit".to_vec()).unwrap();
        let scope = super::super::ProjectionRecordScope::new(
            topology,
            partition,
            "TodoView",
            b"todo-1".to_vec(),
        )
        .unwrap();
        let absent = ResolvedProjectionObligation {
            projector: "todos".into(),
            model: "TodoView".into(),
            key: super::super::ResolvedProjectionKey { fields: Vec::new() },
            partition: None,
            scope,
        };
        let explicit_null = ResolvedProjectionObligation {
            partition: Some(serde_json::Value::Null),
            ..absent.clone()
        };
        let absent_json = serde_json::to_value(&absent).unwrap();
        let null_json = serde_json::to_value(&explicit_null).unwrap();
        assert!(absent_json.get("partition").is_none());
        assert_eq!(null_json.get("partition"), Some(&serde_json::Value::Null));
        assert_eq!(
            serde_json::from_value::<ResolvedProjectionObligation>(null_json)
                .unwrap()
                .partition,
            Some(serde_json::Value::Null)
        );
    }

    #[test]
    fn same_transaction_replay_semantically_decodes_exact_typed_evidence() {
        let replay = same_transaction_evidence().replay_value();
        SameTransactionProjectionEvidence::validate_replay_value(&replay).unwrap();
    }

    #[test]
    fn same_transaction_replay_rejects_version_and_identity_digest_tampering() {
        let valid = same_transaction_evidence().replay_value();
        let mut cases = Vec::new();

        let mut unsupported_version = valid.clone();
        unsupported_version["version"] = serde_json::json!(2);
        cases.push(("version", unsupported_version));

        let mut malformed_topology_digest = valid.clone();
        malformed_topology_digest["records"][0]["revision"]["scope"]["topology_digest"] =
            serde_json::json!("00");
        cases.push(("topology digest", malformed_topology_digest));

        let mut mismatched_topology = valid.clone();
        mismatched_topology["records"][0]["revision"]["scope"]["topology_digest"] =
            serde_json::json!("08".repeat(32));
        cases.push(("topology identity", mismatched_topology));

        let mut mismatched_partition_digest = valid.clone();
        mismatched_partition_digest["records"][0]["revision"]["scope"]["partition_digest"] =
            serde_json::json!("00".repeat(32));
        cases.push(("partition digest", mismatched_partition_digest));

        let mut mismatched_key_digest = valid.clone();
        mismatched_key_digest["records"][0]["revision"]["scope"]["key_digest"] =
            serde_json::json!("00".repeat(32));
        cases.push(("key digest", mismatched_key_digest));

        let mut noncanonical_key = valid;
        noncanonical_key["records"][0]["revision"]["scope"]["key"] = serde_json::json!("AA");
        cases.push(("canonical key", noncanonical_key));

        for (case, replay) in cases {
            assert!(
                SameTransactionProjectionEvidence::validate_replay_value(&replay).is_err(),
                "{case} tampering must be rejected"
            );
        }
    }

    #[test]
    fn same_transaction_replay_rejects_cross_component_semantic_drift() {
        let valid = same_transaction_evidence().replay_value();
        let mut cases = Vec::new();

        let mut zero_revision = valid.clone();
        zero_revision["records"][0]["revision"]["revision"] = serde_json::json!(0);
        cases.push(("zero revision", zero_revision));

        let mut mismatched_revision = valid.clone();
        mismatched_revision["changes"][0]["revision"]["revision"] = serde_json::json!(2);
        cases.push(("mismatched revision", mismatched_revision));

        let mut mismatched_cursor = valid.clone();
        mismatched_cursor["observations"][0]["change"]["position"] = serde_json::json!(2);
        cases.push(("mismatched cursor", mismatched_cursor));

        let mut cursor_scope_drift = valid.clone();
        let other_topology_digest = serde_json::json!("08".repeat(32));
        cursor_scope_drift["records"][0]["change"]["topology_digest"] =
            other_topology_digest.clone();
        cursor_scope_drift["changes"][0]["cursor"]["topology_digest"] =
            other_topology_digest.clone();
        cursor_scope_drift["observations"][0]["change"]["topology_digest"] = other_topology_digest;
        cases.push(("cursor/scope topology drift", cursor_scope_drift));

        let mut mismatched_causation = valid.clone();
        mismatched_causation["observations"][0]["causation_id"] = serde_json::json!("other-cause");
        cases.push(("mismatched causation", mismatched_causation));

        let mut tombstone = valid.clone();
        tombstone["records"][0]["tombstone"] = serde_json::json!(true);
        cases.push(("tombstone", tombstone));

        let mut wrong_change_kind = valid.clone();
        wrong_change_kind["changes"][0]["kind"] = serde_json::json!("record_delete");
        cases.push(("wrong change kind", wrong_change_kind));

        let mut duplicate_record = valid;
        let record = duplicate_record["records"][0].clone();
        duplicate_record["records"]
            .as_array_mut()
            .unwrap()
            .push(record);
        cases.push(("duplicate record", duplicate_record));

        for (case, replay) in cases {
            assert!(
                SameTransactionProjectionEvidence::validate_replay_value(&replay).is_err(),
                "{case} must be rejected"
            );
        }
    }

    #[test]
    fn failure_batch_validation_rechecks_all_mutable_identity_and_payload_fields() {
        let valid = failure_batch();
        valid.validate().unwrap();

        let mut cases = Vec::new();
        let mut empty_failure_id = valid.clone();
        empty_failure_id.failure_id.clear();
        cases.push(("empty failure ID", empty_failure_id));

        let mut invalid_failure_code = valid.clone();
        invalid_failure_code.failure_code = "decode error".into();
        cases.push(("invalid failure code", invalid_failure_code));

        let mut empty_details = valid.clone();
        empty_details.failure_bytes.clear();
        empty_details.failure_digest =
            ProjectionFailureBatch::fingerprint_bytes(&empty_details.failure_bytes);
        cases.push(("empty failure details", empty_details));

        let mut oversized_details = valid.clone();
        oversized_details.failure_bytes = vec![0; MAX_FAILURE_DETAIL_BYTES + 1];
        oversized_details.failure_digest =
            ProjectionFailureBatch::fingerprint_bytes(&oversized_details.failure_bytes);
        cases.push(("oversized failure details", oversized_details));

        let mut mismatched_digest = valid.clone();
        mismatched_digest.failure_bytes[0] ^= 0xff;
        cases.push(("mismatched failure digest", mismatched_digest));

        let mut empty_message = valid.clone();
        empty_message.input.message_id.clear();
        cases.push(("empty input message ID", empty_message));

        let mut invalid_causation = valid;
        invalid_causation.input.causation_id = "cause\n2".into();
        cases.push(("invalid input causation ID", invalid_causation));

        for (case, batch) in cases {
            assert!(batch.validate().is_err(), "{case} must be rejected");
        }
    }
}
