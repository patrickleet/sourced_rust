#![expect(
    clippy::manual_async_fn,
    reason = "trait impls preserve the ProjectionProtocolStore Send bounds"
)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;

use super::InMemoryRepository;
use crate::projection_protocol::{
    ProjectionChange, ProjectionChangeCursor, ProjectionChangeKind, ProjectionChangeRead,
    ProjectionChangeRetention, ProjectionCheckpoint, ProjectionCommitBatch,
    ProjectionCommitOutcome, ProjectionCommitResult, ProjectionEpoch, ProjectionFailure,
    ProjectionFailureBatch, ProjectionFailureLocation, ProjectionGeneration, ProjectionInputCursor,
    ProjectionInputDisposition, ProjectionInputFingerprint, ProjectionLiveRecordBatch,
    ProjectionLiveRecordBatchRequest, ProjectionMutationKind, ProjectionObligationEvidence,
    ProjectionObligationEvidenceBatch, ProjectionObligationEvidenceBatchRequest,
    ProjectionObservation, ProjectionObservationKind, ProjectionObservationTarget,
    ProjectionPartition, ProjectionPartitionRuntimeState, ProjectionPendingRetry,
    ProjectionProtocolError, ProjectionProtocolStore, ProjectionQuerySnapshot,
    ProjectionQuerySnapshotBatch, ProjectionQuerySnapshotBatchRequest,
    ProjectionQuerySnapshotRequest, ProjectionRecordExpectation, ProjectionRecordMetadata,
    ProjectionRecordScope, ProjectionSource, ProjectorTopologyId, RecordRevision,
    RevisionComparison, SameTransactionProjectionBatch, SameTransactionProjectionEvidence,
    TrustedProjectionInput, MAX_PROJECTION_POSITION,
};
use crate::read_model::in_memory::{
    apply_read_model_write_plan, relational_storage_key, StoredRow,
};
use crate::repository::RepositoryError;
use crate::table::{TableMutation, TableStoreError, TableWritePlan};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct PartitionKey {
    topology: ProjectorTopologyId,
    partition: ProjectionPartition,
}

impl PartitionKey {
    fn new(topology: &ProjectorTopologyId, partition: &ProjectionPartition) -> Self {
        Self {
            topology: topology.clone(),
            partition: partition.clone(),
        }
    }

    fn from_input(input: &ProjectionInputCursor) -> Self {
        Self::new(input.topology(), input.projection_partition())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct InputKey {
    partition: PartitionKey,
    source: ProjectionSource,
    generation: ProjectionGeneration,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct SourceCapabilityKey {
    partition: PartitionKey,
    source: ProjectionSource,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct CursorReceiptKey {
    partition: PartitionKey,
    source: ProjectionSource,
    source_epoch: ProjectionEpoch,
    source_position: u64,
    generation: ProjectionGeneration,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct CursorIdentityKey {
    partition: PartitionKey,
    source: ProjectionSource,
    source_epoch: ProjectionEpoch,
    source_position: u64,
}

impl CursorReceiptKey {
    fn new(cursor: &ProjectionInputCursor, generation: ProjectionGeneration) -> Self {
        Self {
            partition: PartitionKey::from_input(cursor),
            source: cursor.source().clone(),
            source_epoch: cursor.epoch().clone(),
            source_position: cursor.position(),
            generation,
        }
    }
}

impl CursorIdentityKey {
    fn new(cursor: &ProjectionInputCursor) -> Self {
        Self {
            partition: PartitionKey::from_input(cursor),
            source: cursor.source().clone(),
            source_epoch: cursor.epoch().clone(),
            source_position: cursor.position(),
        }
    }
}

impl SourceCapabilityKey {
    fn new(cursor: &ProjectionInputCursor) -> Self {
        Self {
            partition: PartitionKey::from_input(cursor),
            source: cursor.source().clone(),
        }
    }
}

impl InputKey {
    fn new(cursor: &ProjectionInputCursor, generation: ProjectionGeneration) -> Self {
        Self {
            partition: PartitionKey::from_input(cursor),
            source: cursor.source().clone(),
            generation,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct MessageKey {
    topology: ProjectorTopologyId,
    message_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct GenerationKey {
    partition: PartitionKey,
    generation: ProjectionGeneration,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct OwnershipKey {
    partition: PartitionKey,
    model: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct RegisteredModelKey {
    topology: ProjectorTopologyId,
    model: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ObservationKey {
    causation_id: String,
    scope: ProjectionRecordScope,
    kind: ProjectionObservationKind,
}

#[derive(Clone)]
struct StoredInput {
    cursor: ProjectionInputCursor,
    fingerprint: ProjectionInputFingerprint,
    message_id: String,
    causation_id: String,
    checkpoint: ProjectionCheckpoint,
    gap_free: bool,
}

#[derive(Clone)]
struct MessageIdentity {
    cursor: ProjectionInputCursor,
    fingerprint: ProjectionInputFingerprint,
    causation_id: String,
    gap_free: bool,
}

#[derive(Clone)]
struct InputIdentity {
    fingerprint: ProjectionInputFingerprint,
    message_id: String,
    causation_id: String,
    gap_free: bool,
}

#[derive(Clone)]
struct AppliedInputReceipt {
    fingerprint: ProjectionInputFingerprint,
    message_id: String,
    causation_id: String,
    gap_free: bool,
    checkpoint: ProjectionCheckpoint,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GenerationLineage {
    retry_of_generation: Option<ProjectionGeneration>,
    retry_of_failure_id: Option<String>,
}

impl GenerationLineage {
    fn initial() -> Self {
        Self {
            retry_of_generation: None,
            retry_of_failure_id: None,
        }
    }

    fn retry_of(generation: ProjectionGeneration, failure_id: String) -> Self {
        Self {
            retry_of_generation: Some(generation),
            retry_of_failure_id: Some(failure_id),
        }
    }
}

#[derive(Clone)]
struct FailedInputFence {
    cursor: ProjectionInputCursor,
    fingerprint: ProjectionInputFingerprint,
    message_id: String,
    causation_id: String,
    generation: ProjectionGeneration,
    gap_free: bool,
}

impl FailedInputFence {
    fn from_input(input: &TrustedProjectionInput) -> Self {
        Self {
            cursor: input.cursor.clone(),
            fingerprint: input.fingerprint,
            message_id: input.message_id.clone(),
            causation_id: input.causation_id.clone(),
            generation: input.generation,
            gap_free: input.gap_free,
        }
    }

    fn matches_retry(&self, input: &TrustedProjectionInput) -> bool {
        self.cursor == input.cursor
            && self.fingerprint == input.fingerprint
            && self.message_id == input.message_id
            && self.causation_id == input.causation_id
            && self.gap_free == input.gap_free
    }
}

#[derive(Clone)]
struct PartitionState {
    active_generation: ProjectionGeneration,
    change_epoch: ProjectionEpoch,
    change_head: u64,
    compacted_through: u64,
    stopped_failure_id: Option<String>,
    pending_retry_failure_id: Option<String>,
    changes: BTreeMap<u64, ProjectionChange>,
}

struct PendingChange {
    kind: ProjectionChangeKind,
    causation_id: String,
    observation_kind: Option<ProjectionObservationKind>,
    scope: Option<ProjectionRecordScope>,
    revision: Option<RecordRevision>,
    failure_id: Option<String>,
}

impl PartitionState {
    fn new(change_epoch: ProjectionEpoch) -> Self {
        Self {
            active_generation: ProjectionGeneration::initial(),
            change_epoch,
            change_head: 0,
            compacted_through: 0,
            stopped_failure_id: None,
            pending_retry_failure_id: None,
            changes: BTreeMap::new(),
        }
    }
}

/// Dev-only in-memory representation of the durable projection protocol.
///
/// The state is cloned before every transaction. This intentionally favors
/// simple, auditable atomicity over throughput: the production SQL adapters use
/// database transactions, while this adapter is primarily for tests and local
/// development.
#[derive(Clone, Default)]
pub(super) struct InMemoryProjectionProtocolState {
    partitions: HashMap<PartitionKey, PartitionState>,
    generations: HashMap<GenerationKey, GenerationLineage>,
    inputs: HashMap<InputKey, StoredInput>,
    input_identities: HashMap<CursorIdentityKey, InputIdentity>,
    messages: HashMap<MessageKey, MessageIdentity>,
    applied_receipts: HashMap<CursorReceiptKey, AppliedInputReceipt>,
    registered_topologies: HashSet<ProjectorTopologyId>,
    registered_models: HashMap<RegisteredModelKey, String>,
    authoritative_table_owners: HashMap<String, RegisteredModelKey>,
    ownership: HashMap<OwnershipKey, String>,
    gap_free_capabilities: HashMap<SourceCapabilityKey, bool>,
    records: HashMap<ProjectionRecordScope, ProjectionRecordMetadata>,
    observations: HashMap<ObservationKey, ProjectionObservation>,
    failures: HashMap<String, ProjectionFailure>,
    failure_inputs: HashMap<String, FailedInputFence>,
}

pub(super) fn reject_causal_owned_plans(
    causal_tables: &HashSet<String>,
    plans: &[TableWritePlan],
) -> Result<(), TableStoreError> {
    if let Some(table) = plans
        .iter()
        .flat_map(|plan| &plan.mutations)
        .map(TableMutation::table_name)
        .find(|table| causal_tables.contains(*table))
    {
        return Err(TableStoreError::CausalWriteRequired {
            table: table.to_string(),
        });
    }
    Ok(())
}

enum InputDisposition {
    New,
    Duplicate(ProjectionCheckpoint),
    Stale(ProjectionCheckpoint),
}

impl InMemoryProjectionProtocolState {
    fn ensure_live_record_identity_available(
        &self,
        metadata: &ProjectionRecordMetadata,
    ) -> Result<(), ProjectionProtocolError> {
        if metadata.tombstone {
            return Ok(());
        }
        let candidate = metadata.revision.scope();
        for (scope, stored) in &self.records {
            if stored.tombstone
                || scope == candidate
                || scope.topology() != candidate.topology()
                || scope.model() != candidate.model()
                || scope.key_digest() != candidate.key_digest()
            {
                continue;
            }
            if scope.canonical_key_bytes() != candidate.canonical_key_bytes() {
                return Err(ProjectionProtocolError::InvalidBatch(format!(
                    "projection live-record key digest collision for model `{}`",
                    candidate.model()
                )));
            }
            return Err(ProjectionProtocolError::InvalidBatch(format!(
                "projection live record for model `{}` is already owned by another partition",
                candidate.model()
            )));
        }
        Ok(())
    }

    fn require_registered_topology(
        &self,
        topology: &ProjectorTopologyId,
    ) -> Result<(), ProjectionProtocolError> {
        if self.registered_topologies.contains(topology) {
            Ok(())
        } else {
            Err(ProjectionProtocolError::InvalidBatch(
                "projector topology was not bootstrapped before projector traffic".into(),
            ))
        }
    }

    fn ensure_partition(
        &mut self,
        key: &PartitionKey,
        change_epoch: &ProjectionEpoch,
    ) -> Result<(), ProjectionProtocolError> {
        match self.partitions.get(key) {
            Some(partition) if &partition.change_epoch != change_epoch => {
                return Err(ProjectionProtocolError::ScopeMismatch {
                    field: "projection change epoch",
                });
            }
            Some(_) => {}
            None => {
                self.partitions
                    .insert(key.clone(), PartitionState::new(change_epoch.clone()));
                self.generations.insert(
                    GenerationKey {
                        partition: key.clone(),
                        generation: ProjectionGeneration::initial(),
                    },
                    GenerationLineage::initial(),
                );
            }
        }
        Ok(())
    }

    fn validate_partition(
        &self,
        key: &PartitionKey,
        input: &TrustedProjectionInput,
        change_epoch: &ProjectionEpoch,
    ) -> Result<(), ProjectionProtocolError> {
        self.require_registered_topology(input.cursor.topology())?;
        self.validate_known_input_identity(input)?;
        let Some(partition) = self.partitions.get(key) else {
            if input.generation != ProjectionGeneration::initial() {
                return Err(ProjectionProtocolError::GenerationFenced {
                    expected: ProjectionGeneration::initial().get(),
                    actual: input.generation.get(),
                });
            }
            self.validate_gap_free(&input.cursor, input.gap_free)?;
            self.validate_input_identity(input)?;
            return Ok(());
        };
        if &partition.change_epoch != change_epoch {
            return Err(ProjectionProtocolError::ScopeMismatch {
                field: "projection change epoch",
            });
        }
        if input.generation != partition.active_generation {
            return Err(ProjectionProtocolError::GenerationFenced {
                expected: partition.active_generation.get(),
                actual: input.generation.get(),
            });
        }
        if !self.generations.contains_key(&GenerationKey {
            partition: key.clone(),
            generation: input.generation,
        }) {
            return Err(ProjectionProtocolError::InvalidBatch(format!(
                "active projection generation {} has no durable lineage",
                input.generation.get()
            )));
        }
        self.validate_gap_free(&input.cursor, input.gap_free)?;
        self.validate_input_identity(input)?;
        if let Some(failure_id) = &partition.stopped_failure_id {
            return Err(ProjectionProtocolError::PartitionStopped {
                failure_id: failure_id.clone(),
            });
        }
        Ok(())
    }

    fn validate_known_input_identity(
        &self,
        input: &TrustedProjectionInput,
    ) -> Result<(), ProjectionProtocolError> {
        let cursor_known = self
            .input_identities
            .contains_key(&CursorIdentityKey::new(&input.cursor));
        let message_known = self.messages.contains_key(&MessageKey {
            topology: input.cursor.topology().clone(),
            message_id: input.message_id.clone(),
        });
        let source_capability_known = self
            .gap_free_capabilities
            .contains_key(&SourceCapabilityKey::new(&input.cursor));
        if cursor_known || message_known || source_capability_known {
            // Exact cursor corruption wins over message reuse; both are durable
            // generation-independent identities. The source's fixed gap-free
            // capability has the same precedence. An entirely unknown
            // old-generation input is still rejected as GenerationFenced below.
            self.validate_input_identity(input)?;
            self.validate_gap_free(&input.cursor, input.gap_free)?;
        }
        Ok(())
    }

    fn validate_pending_retry(
        &self,
        key: &PartitionKey,
        input: &TrustedProjectionInput,
    ) -> Result<(), ProjectionProtocolError> {
        let Some(partition) = self.partitions.get(key) else {
            return Ok(());
        };
        if let Some(failure_id) = &partition.pending_retry_failure_id {
            let Some(fence) = self.failure_inputs.get(failure_id) else {
                return Err(ProjectionProtocolError::InvalidBatch(format!(
                    "pending projection retry `{failure_id}` has no exact input fence"
                )));
            };
            if fence.cursor != input.cursor {
                return Err(ProjectionProtocolError::IncomparableInput);
            }
            if !fence.matches_retry(input) {
                return Err(ProjectionProtocolError::InputCorruption);
            }
        }
        Ok(())
    }

    fn validate_input_identity(
        &self,
        input: &TrustedProjectionInput,
    ) -> Result<(), ProjectionProtocolError> {
        if self
            .input_identities
            .get(&CursorIdentityKey::new(&input.cursor))
            .is_some_and(|identity| {
                identity.fingerprint != input.fingerprint
                    || identity.message_id != input.message_id
                    || identity.causation_id != input.causation_id
                    || identity.gap_free != input.gap_free
            })
        {
            return Err(ProjectionProtocolError::InputCorruption);
        }
        self.reject_reused_message(
            &input.cursor,
            input.fingerprint,
            &input.message_id,
            &input.causation_id,
            input.gap_free,
        )
    }

    fn persist_input_identity(&mut self, input: &TrustedProjectionInput) {
        self.input_identities
            .entry(CursorIdentityKey::new(&input.cursor))
            .or_insert_with(|| InputIdentity {
                fingerprint: input.fingerprint,
                message_id: input.message_id.clone(),
                causation_id: input.causation_id.clone(),
                gap_free: input.gap_free,
            });
        self.messages
            .entry(MessageKey {
                topology: input.cursor.topology().clone(),
                message_id: input.message_id.clone(),
            })
            .or_insert_with(|| MessageIdentity {
                cursor: input.cursor.clone(),
                fingerprint: input.fingerprint,
                causation_id: input.causation_id.clone(),
                gap_free: input.gap_free,
            });
    }

    fn classify_input(
        &self,
        cursor: &ProjectionInputCursor,
        fingerprint: ProjectionInputFingerprint,
        message_id: &str,
        causation_id: &str,
        generation: ProjectionGeneration,
        gap_free: bool,
    ) -> Result<InputDisposition, ProjectionProtocolError> {
        self.validate_gap_free(cursor, gap_free)?;
        let candidate = TrustedProjectionInput {
            cursor: cursor.clone(),
            fingerprint,
            message_id: message_id.to_string(),
            causation_id: causation_id.to_string(),
            generation,
            gap_free,
        };
        self.validate_input_identity(&candidate)?;
        if let Some(receipt) = self
            .applied_receipts
            .get(&CursorReceiptKey::new(cursor, generation))
        {
            if receipt.fingerprint == fingerprint
                && receipt.message_id == message_id
                && receipt.causation_id == causation_id
                && receipt.gap_free == gap_free
            {
                return Ok(InputDisposition::Duplicate(receipt.checkpoint.clone()));
            }
            return Err(ProjectionProtocolError::InputCorruption);
        }
        let input_key = InputKey::new(cursor, generation);
        let Some(previous) = self.inputs.get(&input_key) else {
            self.reject_reused_message(cursor, fingerprint, message_id, causation_id, gap_free)?;
            return Ok(InputDisposition::New);
        };

        match cursor.compare_position(&previous.cursor) {
            RevisionComparison::Equal
                if fingerprint != previous.fingerprint
                    || message_id != previous.message_id
                    || causation_id != previous.causation_id
                    || gap_free != previous.gap_free =>
            {
                Err(ProjectionProtocolError::InputCorruption)
            }
            RevisionComparison::Equal => {
                Ok(InputDisposition::Duplicate(previous.checkpoint.clone()))
            }
            RevisionComparison::Older => {
                self.reject_reused_message(
                    cursor,
                    fingerprint,
                    message_id,
                    causation_id,
                    gap_free,
                )?;
                Ok(InputDisposition::Stale(previous.checkpoint.clone()))
            }
            RevisionComparison::Incomparable => Err(ProjectionProtocolError::IncomparableInput),
            RevisionComparison::Newer => {
                self.reject_reused_message(
                    cursor,
                    fingerprint,
                    message_id,
                    causation_id,
                    gap_free,
                )?;
                if gap_free
                    && cursor.position()
                        != checked_next(previous.cursor.position(), "gap-free projection input")?
                {
                    return Err(ProjectionProtocolError::IncomparableInput);
                }
                Ok(InputDisposition::New)
            }
        }
    }

    fn reject_reused_message(
        &self,
        cursor: &ProjectionInputCursor,
        fingerprint: ProjectionInputFingerprint,
        message_id: &str,
        causation_id: &str,
        gap_free: bool,
    ) -> Result<(), ProjectionProtocolError> {
        let key = MessageKey {
            topology: cursor.topology().clone(),
            message_id: message_id.to_string(),
        };
        if let Some(previous) = self.messages.get(&key) {
            if previous.cursor != *cursor
                || previous.fingerprint != fingerprint
                || previous.causation_id != causation_id
                || previous.gap_free != gap_free
            {
                return Err(ProjectionProtocolError::MessageIdReuse {
                    message_id: message_id.to_string(),
                });
            }
        }
        Ok(())
    }

    fn validate_gap_free(
        &self,
        cursor: &ProjectionInputCursor,
        gap_free: bool,
    ) -> Result<(), ProjectionProtocolError> {
        if self
            .gap_free_capabilities
            .get(&SourceCapabilityKey::new(cursor))
            .is_some_and(|registered| *registered != gap_free)
        {
            return Err(ProjectionProtocolError::InputCorruption);
        }
        Ok(())
    }

    fn has_exact_message_identity(&self, input: &TrustedProjectionInput) -> bool {
        self.messages
            .get(&MessageKey {
                topology: input.cursor.topology().clone(),
                message_id: input.message_id.clone(),
            })
            .is_some_and(|identity| {
                identity.cursor == input.cursor
                    && identity.fingerprint == input.fingerprint
                    && identity.causation_id == input.causation_id
                    && identity.gap_free == input.gap_free
            })
    }

    fn register_ownership(
        &mut self,
        partition: &PartitionKey,
        batch: &ProjectionCommitBatch,
    ) -> Result<(), ProjectionProtocolError> {
        let mut declared_models = HashMap::new();
        let mut declared_tables = HashMap::new();
        for declaration in &batch.ownership {
            let registered_key = RegisteredModelKey {
                topology: partition.topology.clone(),
                model: declaration.model.clone(),
            };
            match self.registered_models.get(&registered_key) {
                Some(table) if table == &declaration.table => {}
                Some(table) => {
                    return Err(ProjectionProtocolError::InvalidBatch(format!(
                        "projection model `{}` was bootstrapped for table `{table}`, not `{}`",
                        declaration.model, declaration.table
                    )));
                }
                None => {
                    return Err(ProjectionProtocolError::InvalidBatch(format!(
                        "projection model `{}` was not registered before projector traffic",
                        declaration.model
                    )));
                }
            }
            if self.authoritative_table_owners.get(&declaration.table) != Some(&registered_key) {
                return Err(ProjectionProtocolError::InvalidBatch(format!(
                    "projection table `{}` does not have model `{}` in this topology as its global authoritative owner",
                    declaration.table, declaration.model
                )));
            }
            if let Some(previous) =
                declared_models.insert(declaration.model.as_str(), declaration.table.as_str())
            {
                if previous != declaration.table {
                    return Err(ProjectionProtocolError::InvalidBatch(format!(
                        "projection model `{}` declares both table `{previous}` and `{}`",
                        declaration.model, declaration.table
                    )));
                }
            }
            if let Some(previous) =
                declared_tables.insert(declaration.table.as_str(), declaration.model.as_str())
            {
                if previous != declaration.model {
                    return Err(ProjectionProtocolError::InvalidBatch(format!(
                        "projection table `{}` is declared by both model `{previous}` and `{}`",
                        declaration.table, declaration.model
                    )));
                }
            }

            let key = OwnershipKey {
                partition: partition.clone(),
                model: declaration.model.clone(),
            };
            if let Some(previous) = self.ownership.get(&key) {
                if previous != &declaration.table {
                    return Err(ProjectionProtocolError::InvalidBatch(format!(
                        "projection model `{}` is already bound to table `{previous}`",
                        declaration.model
                    )));
                }
            }
            if let Some((other, _)) = self.ownership.iter().find(|(other, table)| {
                other.partition == *partition
                    && *table == &declaration.table
                    && other.model != declaration.model
            }) {
                return Err(ProjectionProtocolError::InvalidBatch(format!(
                    "projection table `{}` is already bound to model `{}`",
                    declaration.table, other.model
                )));
            }
            self.ownership.insert(key, declaration.table.clone());
        }

        for mutation in &batch.mutations {
            let table = self.owned_table(partition, &mutation.scope)?;
            if table != mutation.mutation.table_name() {
                return Err(ProjectionProtocolError::InvalidBatch(format!(
                    "projection model `{}` owns table `{table}`, but its mutation targets `{}`",
                    mutation.scope.model(),
                    mutation.mutation.table_name()
                )));
            }
            if table_model_name(&mutation.mutation) != mutation.scope.model() {
                return Err(ProjectionProtocolError::InvalidBatch(format!(
                    "projection record model `{}` does not match table schema model `{}`",
                    mutation.scope.model(),
                    table_model_name(&mutation.mutation)
                )));
            }
        }
        for observation in &batch.observations {
            self.owned_table(partition, observation.scope())?;
        }
        Ok(())
    }

    fn register_same_transaction_ownership(
        &mut self,
        partition: &PartitionKey,
        batch: &SameTransactionProjectionBatch,
    ) -> Result<(), ProjectionProtocolError> {
        let declaration = batch
            .ownership
            .first()
            .expect("direct projection batch validation requires one owner");
        let registered_key = RegisteredModelKey {
            topology: partition.topology.clone(),
            model: declaration.model.clone(),
        };
        match self.registered_models.get(&registered_key) {
            Some(table) if table == &declaration.table => {}
            Some(table) => {
                return Err(ProjectionProtocolError::InvalidBatch(format!(
                    "projection model `{}` was bootstrapped for table `{table}`, not `{}`",
                    declaration.model, declaration.table
                )));
            }
            None => {
                return Err(ProjectionProtocolError::InvalidBatch(format!(
                    "projection model `{}` was not registered before direct projection traffic",
                    declaration.model
                )));
            }
        }
        if self.authoritative_table_owners.get(&declaration.table) != Some(&registered_key) {
            return Err(ProjectionProtocolError::InvalidBatch(format!(
                "projection table `{}` does not have model `{}` in this topology as its global authoritative owner",
                declaration.table, declaration.model
            )));
        }

        let ownership_key = OwnershipKey {
            partition: partition.clone(),
            model: declaration.model.clone(),
        };
        if let Some(previous) = self.ownership.get(&ownership_key) {
            if previous != &declaration.table {
                return Err(ProjectionProtocolError::InvalidBatch(format!(
                    "projection model `{}` is already bound to table `{previous}`",
                    declaration.model
                )));
            }
        }
        if let Some((other, _)) = self.ownership.iter().find(|(other, table)| {
            other.partition == *partition
                && *table == &declaration.table
                && other.model != declaration.model
        }) {
            return Err(ProjectionProtocolError::InvalidBatch(format!(
                "projection table `{}` is already bound to model `{}`",
                declaration.table, other.model
            )));
        }
        self.ownership
            .insert(ownership_key, declaration.table.clone());

        let mutation = batch
            .mutations
            .first()
            .expect("direct projection batch validation requires one mutation");
        let table = self.owned_table(partition, &mutation.scope)?;
        if table != mutation.mutation.table_name()
            || table_model_name(&mutation.mutation) != mutation.scope.model()
        {
            return Err(ProjectionProtocolError::InvalidBatch(format!(
                "direct projection record model `{}` does not own staged table `{}`",
                mutation.scope.model(),
                mutation.mutation.table_name()
            )));
        }
        Ok(())
    }

    fn owned_table<'a>(
        &'a self,
        partition: &PartitionKey,
        scope: &ProjectionRecordScope,
    ) -> Result<&'a str, ProjectionProtocolError> {
        self.ownership
            .get(&OwnershipKey {
                partition: partition.clone(),
                model: scope.model().to_string(),
            })
            .map(String::as_str)
            .ok_or_else(|| {
                ProjectionProtocolError::InvalidBatch(format!(
                    "projection model `{}` has no causal ownership declaration",
                    scope.model()
                ))
            })
    }

    fn next_record(
        &self,
        scope: &ProjectionRecordScope,
        expectation: &ProjectionRecordExpectation,
        kind: ProjectionMutationKind,
    ) -> Result<(RecordRevision, bool), ProjectionProtocolError> {
        let current = self.records.get(scope);
        match (expectation, current, kind) {
            (ProjectionRecordExpectation::Missing, None, ProjectionMutationKind::Upsert) => {
                Ok((RecordRevision::new(scope.clone(), 1, 1)?, false))
            }
            (ProjectionRecordExpectation::Missing, Some(metadata), _) if metadata.tombstone => {
                Err(ProjectionProtocolError::RecordTombstoned {
                    model: scope.model().to_string(),
                })
            }
            (ProjectionRecordExpectation::Missing, Some(_), _) => {
                Err(ProjectionProtocolError::RecordAlreadyExists {
                    model: scope.model().to_string(),
                })
            }
            (ProjectionRecordExpectation::Exact(_), None, _) => {
                Err(ProjectionProtocolError::RecordMissing {
                    model: scope.model().to_string(),
                })
            }
            (ProjectionRecordExpectation::Exact(expected), Some(metadata), _) => {
                if expected != &metadata.revision {
                    return Err(ProjectionProtocolError::RecordRevisionConflict {
                        model: scope.model().to_string(),
                        expected_incarnation: expected.incarnation(),
                        expected_revision: expected.revision(),
                        actual_incarnation: metadata.revision.incarnation(),
                        actual_revision: metadata.revision.revision(),
                    });
                }
                match kind {
                    ProjectionMutationKind::Upsert if metadata.tombstone => {
                        Err(ProjectionProtocolError::RecordTombstoned {
                            model: scope.model().to_string(),
                        })
                    }
                    ProjectionMutationKind::Upsert => Ok((
                        RecordRevision::new(
                            scope.clone(),
                            metadata.revision.incarnation(),
                            checked_next(metadata.revision.revision(), "record revision")?,
                        )?,
                        false,
                    )),
                    ProjectionMutationKind::Delete if metadata.tombstone => {
                        Err(ProjectionProtocolError::RecordTombstoned {
                            model: scope.model().to_string(),
                        })
                    }
                    ProjectionMutationKind::Delete => Ok((
                        RecordRevision::new(
                            scope.clone(),
                            metadata.revision.incarnation(),
                            checked_next(metadata.revision.revision(), "record revision")?,
                        )?,
                        true,
                    )),
                    ProjectionMutationKind::Recreate if !metadata.tombstone => {
                        Err(ProjectionProtocolError::RecreateRequiresTombstone {
                            model: scope.model().to_string(),
                        })
                    }
                    ProjectionMutationKind::Recreate => Ok((
                        RecordRevision::new(
                            scope.clone(),
                            checked_next(metadata.revision.incarnation(), "record incarnation")?,
                            1,
                        )?,
                        false,
                    )),
                }
            }
            (_, _, ProjectionMutationKind::Delete | ProjectionMutationKind::Recreate) => {
                Err(ProjectionProtocolError::InvalidBatch(
                    "delete/recreate requires an exact record expectation".into(),
                ))
            }
        }
    }

    fn validate_physical_record(
        &self,
        scope: &ProjectionRecordScope,
        expectation: &ProjectionRecordExpectation,
        kind: ProjectionMutationKind,
        row_exists: bool,
    ) -> Result<(), ProjectionProtocolError> {
        let should_exist = match (expectation, kind) {
            (ProjectionRecordExpectation::Missing, ProjectionMutationKind::Upsert) => false,
            (ProjectionRecordExpectation::Exact(_), ProjectionMutationKind::Recreate) => false,
            (
                ProjectionRecordExpectation::Exact(_),
                ProjectionMutationKind::Upsert | ProjectionMutationKind::Delete,
            ) => true,
            _ => {
                return Err(ProjectionProtocolError::InvalidBatch(
                    "projection mutation has no valid physical-row expectation".into(),
                ));
            }
        };
        if row_exists == should_exist {
            return Ok(());
        }
        let expected = if should_exist { "present" } else { "absent" };
        let actual = if row_exists { "present" } else { "absent" };
        Err(ProjectionProtocolError::InvalidBatch(format!(
            "projection record `{}` metadata requires its physical row to be {expected}, but it is {actual}",
            scope.model()
        )))
    }

    fn append_change(
        &mut self,
        partition_key: &PartitionKey,
        pending: PendingChange,
    ) -> Result<ProjectionChange, ProjectionProtocolError> {
        let partition = self
            .partitions
            .get_mut(partition_key)
            .expect("partition is initialized before appending changes");
        let position = partition.change_head.checked_add(1).ok_or(
            ProjectionProtocolError::PositionOverflow {
                domain: "projection change",
            },
        )?;
        let cursor = ProjectionChangeCursor::new(
            partition_key.topology.clone(),
            partition_key.partition.clone(),
            partition.change_epoch.clone(),
            position,
        )?;
        let change = ProjectionChange {
            cursor,
            kind: pending.kind,
            causation_id: pending.causation_id,
            observation_kind: pending.observation_kind,
            scope: pending.scope,
            revision: pending.revision,
            failure_id: pending.failure_id,
        };
        partition.change_head = position;
        partition.changes.insert(position, change.clone());
        Ok(change)
    }

    fn compact_changes_through(
        &mut self,
        partition_key: &PartitionKey,
        through: u64,
    ) -> Result<u64, ProjectionProtocolError> {
        let partition = self.partitions.get_mut(partition_key).ok_or_else(|| {
            ProjectionProtocolError::InvalidBatch(
                "cannot compact an unknown projection partition".into(),
            )
        })?;
        if through > partition.change_head {
            return Err(ProjectionProtocolError::InvalidBatch(
                "cannot compact beyond the projection change head".into(),
            ));
        }
        if through <= partition.compacted_through {
            return Ok(partition.compacted_through);
        }
        let expected_removed = through - partition.compacted_through;
        let actual_removed = u64::try_from(
            partition
                .changes
                .range((
                    std::ops::Bound::Excluded(partition.compacted_through),
                    std::ops::Bound::Included(through),
                ))
                .count(),
        )
        .map_err(|_| {
            ProjectionProtocolError::InvalidBatch(
                "projection change compaction count exceeds u64".into(),
            )
        })?;
        if actual_removed != expected_removed {
            return Err(ProjectionProtocolError::InvalidBatch(format!(
                "projection change compaction expected to remove {expected_removed} contiguous entries but found {actual_removed}"
            )));
        }
        partition.changes.retain(|position, _| *position > through);
        partition.compacted_through = through;
        Ok(through)
    }

    fn retain_change_suffix(
        &mut self,
        partition_key: &PartitionKey,
        retention: ProjectionChangeRetention,
    ) -> Result<u64, ProjectionProtocolError> {
        let head = self
            .partitions
            .get(partition_key)
            .ok_or_else(|| {
                ProjectionProtocolError::InvalidBatch(
                    "cannot retain changes for an unknown projection partition".into(),
                )
            })?
            .change_head;
        self.compact_changes_through(
            partition_key,
            head.saturating_sub(retention.max_retained_changes()),
        )
    }
}

/// Stage one compiler-sealed same-transaction projected upsert against cloned
/// row/protocol state. The caller publishes both clones only after every
/// domain, ledger, and projection participant has passed validation.
pub(super) fn stage_same_transaction_projection(
    protocol: &mut InMemoryProjectionProtocolState,
    staged_rows: &mut HashMap<String, StoredRow>,
    batch: &SameTransactionProjectionBatch,
    retention: ProjectionChangeRetention,
) -> Result<SameTransactionProjectionEvidence, ProjectionProtocolError> {
    batch.validate()?;
    protocol.require_registered_topology(&batch.topology)?;

    let partition_key = PartitionKey::new(&batch.topology, &batch.partition);
    protocol.ensure_partition(&partition_key, &batch.change_epoch)?;
    if let Some(failure_id) = protocol
        .partitions
        .get(&partition_key)
        .and_then(|partition| partition.stopped_failure_id.clone())
    {
        return Err(ProjectionProtocolError::PartitionStopped { failure_id });
    }
    if protocol
        .partitions
        .get(&partition_key)
        .is_some_and(|partition| partition.pending_retry_failure_id.is_some())
    {
        return Err(ProjectionProtocolError::IncomparableInput);
    }
    protocol.register_same_transaction_ownership(&partition_key, batch)?;

    let mutation = &batch.mutations[0];
    let lock_key = mutation.mutation.lock_key();
    let row_exists = staged_rows.contains_key(&lock_key);
    let revision = match protocol.records.get(&mutation.scope) {
        None if !row_exists => RecordRevision::new(mutation.scope.clone(), 1, 1)?,
        None => {
            return Err(ProjectionProtocolError::RecordAlreadyExists {
                model: mutation.scope.model().to_string(),
            });
        }
        Some(metadata) if metadata.tombstone => {
            return Err(ProjectionProtocolError::RecordTombstoned {
                model: mutation.scope.model().to_string(),
            });
        }
        Some(_) if !row_exists => {
            return Err(ProjectionProtocolError::RecordMissing {
                model: mutation.scope.model().to_string(),
            });
        }
        Some(metadata) => RecordRevision::new(
            mutation.scope.clone(),
            metadata.revision.incarnation(),
            checked_next(metadata.revision.revision(), "record revision")?,
        )?,
    };

    let change = protocol.append_change(
        &partition_key,
        PendingChange {
            kind: ProjectionChangeKind::RecordUpsert,
            causation_id: batch.causation_id.clone(),
            observation_kind: None,
            scope: Some(mutation.scope.clone()),
            revision: Some(revision.clone()),
            failure_id: None,
        },
    )?;
    let metadata = ProjectionRecordMetadata {
        revision: revision.clone(),
        tombstone: false,
        change: change.cursor.clone(),
    };
    protocol.ensure_live_record_identity_available(&metadata)?;
    protocol
        .records
        .insert(mutation.scope.clone(), metadata.clone());

    apply_read_model_write_plan(
        TableWritePlan::new(vec![mutation.mutation.clone()]),
        staged_rows,
    )?;
    if !staged_rows.contains_key(&lock_key) {
        return Err(ProjectionProtocolError::InvalidBatch(format!(
            "direct projection upsert left model `{}` without a physical row",
            mutation.scope.model()
        )));
    }

    let observation_key = ObservationKey {
        causation_id: batch.causation_id.clone(),
        scope: mutation.scope.clone(),
        kind: ProjectionObservationKind::Record,
    };
    if protocol.observations.contains_key(&observation_key) {
        return Err(ProjectionProtocolError::InvalidBatch(format!(
            "direct projection causation `{}` already observed this record",
            batch.causation_id
        )));
    }
    let observation = ProjectionObservation {
        causation_id: batch.causation_id.clone(),
        kind: ProjectionObservationKind::Record,
        revision: Some(revision),
        scope: mutation.scope.clone(),
        change: change.cursor.clone(),
    };
    protocol
        .observations
        .insert(observation_key, observation.clone());
    protocol.retain_change_suffix(&partition_key, retention)?;

    Ok(SameTransactionProjectionEvidence {
        records: vec![metadata],
        changes: vec![change],
        observations: vec![observation],
    })
}

fn read_projection_query_snapshot_from_state(
    rows: &HashMap<String, StoredRow>,
    protocol: &InMemoryProjectionProtocolState,
    request: &ProjectionQuerySnapshotRequest,
) -> Result<ProjectionQuerySnapshot, ProjectionProtocolError> {
    request.validate()?;
    let registered_key = RegisteredModelKey {
        topology: request.scope.topology().clone(),
        model: request.scope.model().to_string(),
    };
    if protocol
        .registered_models
        .get(&registered_key)
        .map(String::as_str)
        != Some(request.schema.table_name.as_str())
        || protocol
            .authoritative_table_owners
            .get(&request.schema.table_name)
            != Some(&registered_key)
    {
        return Err(ProjectionProtocolError::InvalidBatch(format!(
            "projection query model `{}` is not the registered owner of table `{}`",
            request.scope.model(),
            request.schema.table_name
        )));
    }

    let storage_key = relational_storage_key(&request.schema.table_name, &request.key);
    let row = rows.get(&storage_key).map(|stored| stored.values.clone());
    let record = protocol.records.get(&request.scope).cloned();
    match (row.is_some(), record.as_ref()) {
        (true, None)
        | (
            true,
            Some(ProjectionRecordMetadata {
                tombstone: true, ..
            }),
        ) => {
            return Err(ProjectionProtocolError::RecordAlreadyExists {
                model: request.scope.model().to_string(),
            });
        }
        (
            false,
            Some(ProjectionRecordMetadata {
                tombstone: false, ..
            }),
        ) => {
            return Err(ProjectionProtocolError::RecordMissing {
                model: request.scope.model().to_string(),
            });
        }
        _ => {}
    }

    let partition_key = PartitionKey::new(
        request.scope.topology(),
        request.scope.projection_partition(),
    );
    let partition = protocol.partitions.get(&partition_key);
    if record.is_some() && partition.is_none() {
        return Err(ProjectionProtocolError::InvalidBatch(
            "projection query record exists without partition state".into(),
        ));
    }

    let (change_head, compacted_through) = match partition {
        Some(partition) => {
            if let Some(record) = &record {
                if record.change.epoch() != &partition.change_epoch
                    || record.change.position() > partition.change_head
                {
                    return Err(ProjectionProtocolError::InvalidBatch(
                        "projection query record change lies outside its partition head".into(),
                    ));
                }
            }
            let head = if partition.change_head == 0 {
                None
            } else {
                Some(ProjectionChangeCursor::new(
                    request.scope.topology().clone(),
                    request.scope.projection_partition().clone(),
                    partition.change_epoch.clone(),
                    partition.change_head,
                )?)
            };
            (head, partition.compacted_through)
        }
        None => (None, 0),
    };

    let mut checkpoints = Vec::with_capacity(request.checkpoint_probes.len());
    for probe in &request.checkpoint_probes {
        let stored = protocol.inputs.get(&InputKey {
            partition: partition_key.clone(),
            source: probe.source.clone(),
            generation: probe.generation,
        });
        let checkpoint = match stored {
            Some(stored) => {
                let Some(partition) = partition else {
                    return Err(ProjectionProtocolError::InvalidBatch(
                        "projection checkpoint exists without partition state".into(),
                    ));
                };
                if stored.cursor.epoch() != &probe.epoch {
                    return Err(ProjectionProtocolError::IncomparableInput);
                }
                if stored.checkpoint.change().epoch() != &partition.change_epoch
                    || stored.checkpoint.change().position() > partition.change_head
                {
                    return Err(ProjectionProtocolError::InvalidBatch(
                        "projection query checkpoint lies outside its partition head".into(),
                    ));
                }
                Some(stored.checkpoint.clone())
            }
            None => None,
        };
        checkpoints.push(crate::projection_protocol::ProjectionCheckpointSnapshot {
            probe: probe.clone(),
            checkpoint,
        });
    }

    Ok(ProjectionQuerySnapshot {
        row,
        record,
        checkpoints,
        change_head,
        compacted_through,
    })
}

fn validate_observation_from_state(
    protocol: &InMemoryProjectionProtocolState,
    request: &crate::projection_protocol::ProjectionObligationEvidenceRequest,
    observation: &ProjectionObservation,
) -> Result<(), ProjectionProtocolError> {
    if observation.causation_id != request.causation_id
        || observation.kind != request.kind
        || observation.scope != request.scope
        || observation
            .revision
            .as_ref()
            .is_some_and(|revision| revision.scope() != &request.scope)
        || (request.kind == ProjectionObservationKind::Record) != observation.revision.is_some()
    {
        return Err(ProjectionProtocolError::InvalidBatch(
            "stored projection observation does not match its exact evidence key".into(),
        ));
    }
    let partition = protocol
        .partitions
        .get(&PartitionKey::new(
            request.scope.topology(),
            request.scope.projection_partition(),
        ))
        .ok_or_else(|| {
            ProjectionProtocolError::InvalidBatch(
                "stored projection observation has no partition state".into(),
            )
        })?;
    if observation.change.topology() != request.scope.topology()
        || observation.change.projection_partition() != request.scope.projection_partition()
        || observation.change.epoch() != &partition.change_epoch
        || observation.change.position() > partition.change_head
    {
        return Err(ProjectionProtocolError::InvalidBatch(
            "stored projection observation change lies outside its partition".into(),
        ));
    }
    Ok(())
}

fn validate_failure_from_state(
    protocol: &InMemoryProjectionProtocolState,
    request: &crate::projection_protocol::ProjectionObligationEvidenceRequest,
    failure: &ProjectionFailure,
) -> Result<(), ProjectionProtocolError> {
    if failure.causation_id != request.causation_id
        || failure.input.topology() != request.scope.topology()
        || failure.input.projection_partition() != request.scope.projection_partition()
        || failure.change.topology() != request.scope.topology()
        || failure.change.projection_partition() != request.scope.projection_partition()
    {
        return Err(ProjectionProtocolError::InvalidBatch(
            "stored projection failure does not match its evidence scope".into(),
        ));
    }
    let partition = protocol
        .partitions
        .get(&PartitionKey::new(
            request.scope.topology(),
            request.scope.projection_partition(),
        ))
        .ok_or_else(|| {
            ProjectionProtocolError::InvalidBatch(
                "stored projection failure has no partition state".into(),
            )
        })?;
    if failure.change.epoch() != &partition.change_epoch
        || failure.change.position() > partition.change_head
    {
        return Err(ProjectionProtocolError::InvalidBatch(
            "stored projection failure change lies outside its partition".into(),
        ));
    }
    Ok(())
}

fn read_projection_obligation_evidence_from_state(
    protocol: &InMemoryProjectionProtocolState,
    request: &crate::projection_protocol::ProjectionObligationEvidenceRequest,
) -> Result<ProjectionObligationEvidence, ProjectionProtocolError> {
    request.validate()?;
    let failure = protocol
        .failures
        .values()
        .filter(|failure| {
            failure.causation_id == request.causation_id
                && failure.input.topology() == request.scope.topology()
                && failure.input.projection_partition() == request.scope.projection_partition()
        })
        .min_by_key(|failure| failure.change.position())
        .cloned();
    if let Some(failure) = failure {
        validate_failure_from_state(protocol, request, &failure)?;
        return Ok(ProjectionObligationEvidence::TerminalFailure(failure));
    }

    let observation = protocol
        .observations
        .get(&ObservationKey {
            causation_id: request.causation_id.clone(),
            scope: request.scope.clone(),
            kind: request.kind,
        })
        .cloned();
    match observation {
        Some(observation) => {
            validate_observation_from_state(protocol, request, &observation)?;
            Ok(ProjectionObligationEvidence::Observed(observation))
        }
        None => Ok(ProjectionObligationEvidence::Pending),
    }
}

fn read_projection_live_record_from_state(
    protocol: &InMemoryProjectionProtocolState,
    request: &crate::projection_protocol::ProjectionLiveRecordRequest,
) -> Result<Option<ProjectionRecordMetadata>, ProjectionProtocolError> {
    request.validate()?;
    let registered_key = RegisteredModelKey {
        topology: request.topology.clone(),
        model: request.model().to_string(),
    };
    if protocol
        .registered_models
        .get(&registered_key)
        .map(String::as_str)
        != Some(request.schema.table_name.as_str())
        || protocol
            .authoritative_table_owners
            .get(&request.schema.table_name)
            != Some(&registered_key)
    {
        return Err(ProjectionProtocolError::InvalidBatch(format!(
            "projection live-record model `{}` is not the registered owner of table `{}`",
            request.model(),
            request.schema.table_name
        )));
    }

    let mut live = None;
    for (scope, metadata) in &protocol.records {
        if metadata.tombstone
            || scope.topology() != &request.topology
            || scope.model() != request.model()
            || scope.key_digest() != request.canonical_key_hash
        {
            continue;
        }
        if scope.canonical_key_bytes() != request.canonical_key_bytes {
            return Err(ProjectionProtocolError::InvalidBatch(format!(
                "projection live-record canonical key mismatch for model `{}`",
                request.model()
            )));
        }
        if metadata.revision.scope() != scope {
            return Err(ProjectionProtocolError::InvalidBatch(
                "projection live-record metadata is stored under a different scope".into(),
            ));
        }
        if live.replace(metadata.clone()).is_some() {
            return Err(ProjectionProtocolError::InvalidBatch(format!(
                "projection live-record identity for model `{}` is ambiguous across partitions",
                request.model()
            )));
        }
    }

    if let Some(metadata) = &live {
        let scope = metadata.revision.scope();
        let partition = protocol
            .partitions
            .get(&PartitionKey::new(
                scope.topology(),
                scope.projection_partition(),
            ))
            .ok_or_else(|| {
                ProjectionProtocolError::InvalidBatch(
                    "projection live record has no partition state".into(),
                )
            })?;
        if metadata.change.topology() != scope.topology()
            || metadata.change.projection_partition() != scope.projection_partition()
            || metadata.change.epoch() != &partition.change_epoch
            || metadata.change.position() > partition.change_head
        {
            return Err(ProjectionProtocolError::InvalidBatch(
                "projection live-record change lies outside its partition".into(),
            ));
        }
    }
    Ok(live)
}

impl ProjectionProtocolStore for InMemoryRepository {
    fn register_projection_models<'a>(
        &'a self,
        topology: &'a ProjectorTopologyId,
        ownership: &'a [crate::projection_protocol::ProjectionModelOwnership],
    ) -> impl Future<Output = Result<(), ProjectionProtocolError>> + Send + 'a {
        async move {
            if ownership.is_empty() {
                return Err(ProjectionProtocolError::InvalidBatch(
                    "projection topology bootstrap requires at least one model/table owner".into(),
                ));
            }
            // Serialize bootstrap against raw row writers. Once this returns,
            // every repository-level legacy path observes the causal marker
            // before it can acquire the row map for mutation.
            let rows = self
                .model_store
                .relational_rows
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("projection ownership row fence"))?;
            let mut protocol = self
                .projection_protocol
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("projection ownership write"))?;
            let mut staged = protocol.clone();
            staged.registered_topologies.insert(topology.clone());
            let mut batch_models = HashMap::new();
            let mut batch_tables = HashMap::new();
            for declaration in ownership {
                if let Some(previous) =
                    batch_models.insert(declaration.model.as_str(), declaration.table.as_str())
                {
                    if previous != declaration.table {
                        return Err(ProjectionProtocolError::InvalidBatch(format!(
                            "projection model `{}` declares both table `{previous}` and `{}`",
                            declaration.model, declaration.table
                        )));
                    }
                }
                if let Some(previous) =
                    batch_tables.insert(declaration.table.as_str(), declaration.model.as_str())
                {
                    if previous != declaration.model {
                        return Err(ProjectionProtocolError::InvalidBatch(format!(
                            "projection table `{}` is declared by both model `{previous}` and `{}`",
                            declaration.table, declaration.model
                        )));
                    }
                }
                let key = RegisteredModelKey {
                    topology: topology.clone(),
                    model: declaration.model.clone(),
                };
                if let Some(previous) = staged.authoritative_table_owners.get(&declaration.table) {
                    if previous != &key {
                        return Err(ProjectionProtocolError::InvalidBatch(format!(
                            "projection table `{}` already has authoritative owner `{}` in another topology",
                            declaration.table, previous.model
                        )));
                    }
                } else if rows.keys().any(|storage_key| {
                    storage_key_belongs_to_table(storage_key, &declaration.table)
                }) {
                    return Err(ProjectionProtocolError::InvalidBatch(format!(
                        "projection table `{}` contains rows without causal metadata; rebuild or verified import is required before registration",
                        declaration.table
                    )));
                }
                if let Some(previous) = staged.registered_models.get(&key) {
                    if previous != &declaration.table {
                        return Err(ProjectionProtocolError::InvalidBatch(format!(
                            "projection model `{}` is already registered for table `{previous}`",
                            declaration.model
                        )));
                    }
                }
                if let Some((other, _)) = staged.registered_models.iter().find(|(other, table)| {
                    other.topology == *topology
                        && *table == &declaration.table
                        && other.model != declaration.model
                }) {
                    return Err(ProjectionProtocolError::InvalidBatch(format!(
                        "projection table `{}` is already registered for model `{}`",
                        declaration.table, other.model
                    )));
                }
                staged
                    .registered_models
                    .insert(key.clone(), declaration.table.clone());
                staged
                    .authoritative_table_owners
                    .insert(declaration.table.clone(), key);
            }
            let mut causal_tables = self
                .causal_tables
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("causal table marker write"))?;
            *protocol = staged;
            causal_tables.extend(
                ownership
                    .iter()
                    .map(|declaration| declaration.table.clone()),
            );
            Ok(())
        }
    }

    fn commit_projection(
        &self,
        batch: ProjectionCommitBatch,
    ) -> impl Future<Output = Result<ProjectionCommitResult, ProjectionProtocolError>> + Send + '_
    {
        async move {
            batch.validate()?;
            let partition_key = PartitionKey::from_input(&batch.input.cursor);

            // All projection commits use one lock order: rows, protocol, inbox.
            // Protocol/inbox guards therefore drop before the row guard, so a
            // reader can never observe a row before its revision metadata.
            let mut rows = self
                .model_store
                .relational_rows
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("projection rows write"))?;
            let mut protocol = self
                .projection_protocol
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("projection protocol write"))?;
            let mut inbox = self
                .inbox_store
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("projection inbox write"))?;

            protocol.validate_partition(&partition_key, &batch.input, &batch.change_epoch)?;
            match protocol.classify_input(
                &batch.input.cursor,
                batch.input.fingerprint,
                &batch.input.message_id,
                &batch.input.causation_id,
                batch.input.generation,
                batch.input.gap_free,
            )? {
                InputDisposition::Duplicate(checkpoint) => {
                    return Ok(ProjectionCommitResult::not_applied(
                        ProjectionCommitOutcome::Duplicate,
                        Some(checkpoint),
                    ));
                }
                InputDisposition::Stale(checkpoint) => {
                    return Ok(ProjectionCommitResult::not_applied(
                        ProjectionCommitOutcome::StaleInput,
                        Some(checkpoint),
                    ));
                }
                InputDisposition::New => {
                    protocol.validate_pending_retry(&partition_key, &batch.input)?;
                }
            }

            let receipt = batch.input.inbox_receipt();
            receipt.validate()?;
            if inbox.contains(&(receipt.consumer.clone(), receipt.message_id.clone())) {
                return Err(ProjectionProtocolError::MessageIdReuse {
                    message_id: batch.input.message_id.clone(),
                });
            }

            let mut staged_protocol = protocol.clone();
            let mut staged_inbox = inbox.clone();
            staged_protocol.ensure_partition(&partition_key, &batch.change_epoch)?;
            staged_protocol
                .gap_free_capabilities
                .entry(SourceCapabilityKey::new(&batch.input.cursor))
                .or_insert(batch.input.gap_free);
            staged_protocol.register_ownership(&partition_key, &batch)?;

            let mut touched_rows = HashSet::with_capacity(batch.mutations.len());
            for mutation in &batch.mutations {
                let lock_key = mutation.mutation.lock_key();
                if !touched_rows.insert(lock_key.clone()) {
                    return Err(ProjectionProtocolError::InvalidBatch(format!(
                        "projection batch repeats table row `{lock_key}`"
                    )));
                }
            }
            let mut staged_rows = HashMap::with_capacity(touched_rows.len());
            for key in &touched_rows {
                if let Some(row) = rows.get(key) {
                    staged_rows.insert(key.clone(), row.clone());
                }
            }

            let mut records = Vec::with_capacity(batch.mutations.len());
            let mut changes =
                Vec::with_capacity(batch.mutations.len() + batch.observations.len().max(1));
            for mutation in &batch.mutations {
                let (revision, tombstone) = staged_protocol.next_record(
                    &mutation.scope,
                    &mutation.expectation,
                    mutation.kind,
                )?;
                staged_protocol.validate_physical_record(
                    &mutation.scope,
                    &mutation.expectation,
                    mutation.kind,
                    staged_rows.contains_key(&mutation.mutation.lock_key()),
                )?;
                let kind = match mutation.kind {
                    ProjectionMutationKind::Upsert => ProjectionChangeKind::RecordUpsert,
                    ProjectionMutationKind::Delete => ProjectionChangeKind::RecordDelete,
                    ProjectionMutationKind::Recreate => ProjectionChangeKind::RecordRecreate,
                };
                let change = staged_protocol.append_change(
                    &partition_key,
                    PendingChange {
                        kind,
                        causation_id: batch.input.causation_id.clone(),
                        observation_kind: None,
                        scope: Some(mutation.scope.clone()),
                        revision: Some(revision.clone()),
                        failure_id: None,
                    },
                )?;
                let metadata = ProjectionRecordMetadata {
                    revision,
                    tombstone,
                    change: change.cursor.clone(),
                };
                staged_protocol.ensure_live_record_identity_available(&metadata)?;
                staged_protocol
                    .records
                    .insert(mutation.scope.clone(), metadata.clone());
                records.push(metadata);
                changes.push(change);
            }

            apply_read_model_write_plan(
                TableWritePlan::new(
                    batch
                        .mutations
                        .iter()
                        .map(|mutation| mutation.mutation.clone())
                        .collect(),
                ),
                &mut staged_rows,
            )?;
            for mutation in &batch.mutations {
                let row_exists = staged_rows.contains_key(&mutation.mutation.lock_key());
                let should_exist = !matches!(mutation.kind, ProjectionMutationKind::Delete);
                if row_exists != should_exist {
                    return Err(ProjectionProtocolError::InvalidBatch(format!(
                        "projection table mutation left model `{}` physical row {}",
                        mutation.scope.model(),
                        if row_exists {
                            "present when deletion required absence"
                        } else {
                            "absent when persistence required presence"
                        }
                    )));
                }
            }

            for request in &batch.observations {
                let (scope, revision, staged_change) = match &request.target {
                    ProjectionObservationTarget::StagedRecord(scope) => {
                        let metadata = staged_protocol
                            .records
                            .get(scope)
                            .expect("batch validation requires a staged record");
                        (
                            scope.clone(),
                            Some(metadata.revision.clone()),
                            Some(metadata.change.clone()),
                        )
                    }
                    ProjectionObservationTarget::ExistingRecord(expected) => {
                        let Some(metadata) = staged_protocol.records.get(expected.scope()) else {
                            return Err(ProjectionProtocolError::RecordMissing {
                                model: expected.scope().model().to_string(),
                            });
                        };
                        if &metadata.revision != expected {
                            return Err(ProjectionProtocolError::RecordRevisionConflict {
                                model: expected.scope().model().to_string(),
                                expected_incarnation: expected.incarnation(),
                                expected_revision: expected.revision(),
                                actual_incarnation: metadata.revision.incarnation(),
                                actual_revision: metadata.revision.revision(),
                            });
                        }
                        (expected.scope().clone(), Some(expected.clone()), None)
                    }
                    ProjectionObservationTarget::Dependency(scope) => (scope.clone(), None, None),
                };
                let observation_key = ObservationKey {
                    causation_id: batch.input.causation_id.clone(),
                    scope: scope.clone(),
                    kind: request.kind,
                };
                if staged_protocol.observations.contains_key(&observation_key) {
                    continue;
                }
                let change_cursor = match staged_change {
                    Some(cursor) => cursor,
                    None => {
                        let change = staged_protocol.append_change(
                            &partition_key,
                            PendingChange {
                                kind: ProjectionChangeKind::Observation,
                                causation_id: batch.input.causation_id.clone(),
                                observation_kind: Some(request.kind),
                                scope: Some(scope.clone()),
                                revision: revision.clone(),
                                failure_id: None,
                            },
                        )?;
                        let cursor = change.cursor.clone();
                        changes.push(change);
                        cursor
                    }
                };
                let observation = ProjectionObservation {
                    causation_id: batch.input.causation_id.clone(),
                    kind: request.kind,
                    revision,
                    scope,
                    change: change_cursor,
                };
                staged_protocol
                    .observations
                    .insert(observation_key, observation);
            }

            if changes.is_empty() {
                changes.push(staged_protocol.append_change(
                    &partition_key,
                    PendingChange {
                        kind: ProjectionChangeKind::Checkpoint,
                        causation_id: batch.input.causation_id.clone(),
                        observation_kind: None,
                        scope: None,
                        revision: None,
                        failure_id: None,
                    },
                )?);
            }
            let checkpoint = ProjectionCheckpoint::new(
                batch.input.cursor.clone(),
                changes
                    .last()
                    .expect("every successful projection input emits a change")
                    .cursor
                    .clone(),
                batch.input.gap_free,
            )?;
            let input_key = InputKey::new(&batch.input.cursor, batch.input.generation);
            staged_protocol.inputs.insert(
                input_key,
                StoredInput {
                    cursor: batch.input.cursor.clone(),
                    fingerprint: batch.input.fingerprint,
                    message_id: batch.input.message_id.clone(),
                    causation_id: batch.input.causation_id.clone(),
                    checkpoint: checkpoint.clone(),
                    gap_free: batch.input.gap_free,
                },
            );
            staged_protocol.persist_input_identity(&batch.input);
            staged_protocol.applied_receipts.insert(
                CursorReceiptKey::new(&batch.input.cursor, batch.input.generation),
                AppliedInputReceipt {
                    fingerprint: batch.input.fingerprint,
                    message_id: batch.input.message_id.clone(),
                    causation_id: batch.input.causation_id.clone(),
                    gap_free: batch.input.gap_free,
                    checkpoint: checkpoint.clone(),
                },
            );
            staged_protocol
                .partitions
                .get_mut(&partition_key)
                .expect("successful projection partition is initialized")
                .pending_retry_failure_id = None;
            staged_protocol
                .retain_change_suffix(&partition_key, self.projection_change_retention)?;
            staged_inbox.insert((receipt.consumer, receipt.message_id));

            // No fallible operations below this point. Merge only touched rows,
            // then publish the staged protocol/inbox maps while all guards remain
            // held. The row write guard is declared first and releases last.
            for key in touched_rows {
                match staged_rows.remove(&key) {
                    Some(row) => {
                        rows.insert(key, row);
                    }
                    None => {
                        rows.remove(&key);
                    }
                }
            }
            *protocol = staged_protocol;
            *inbox = staged_inbox;

            Ok(ProjectionCommitResult {
                outcome: ProjectionCommitOutcome::Applied,
                checkpoint: Some(checkpoint),
                records,
                changes,
            })
        }
    }

    fn record_projection_failure(
        &self,
        batch: ProjectionFailureBatch,
    ) -> impl Future<Output = Result<ProjectionFailure, ProjectionProtocolError>> + Send + '_ {
        async move {
            batch.validate()?;
            let partition_key = PartitionKey::from_input(&batch.input.cursor);

            // Keep the same global lock order as successful commits even though
            // failures intentionally do not touch rows.
            let _rows = self
                .model_store
                .relational_rows
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("projection rows failure fence"))?;
            let mut protocol = self
                .projection_protocol
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("projection failure write"))?;
            let mut inbox = self
                .inbox_store
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("projection failure inbox write"))?;

            protocol.require_registered_topology(batch.input.cursor.topology())?;
            protocol.validate_known_input_identity(&batch.input)?;
            if let Some(partition) = protocol.partitions.get(&partition_key) {
                if batch.input.generation != partition.active_generation {
                    return Err(ProjectionProtocolError::GenerationFenced {
                        expected: partition.active_generation.get(),
                        actual: batch.input.generation.get(),
                    });
                }
                protocol.validate_gap_free(&batch.input.cursor, batch.input.gap_free)?;
                protocol.validate_input_identity(&batch.input)?;
                if let Some(stopped_failure_id) = &partition.stopped_failure_id {
                    if stopped_failure_id == &batch.failure_id {
                        if let Some(existing) = protocol.failures.get(stopped_failure_id) {
                            if failure_matches_batch(existing, &batch)
                                && protocol.failure_inputs.get(stopped_failure_id).is_some_and(
                                    |fence| {
                                        fence.generation == batch.input.generation
                                            && fence.matches_retry(&batch.input)
                                    },
                                )
                                && protocol.has_exact_message_identity(&batch.input)
                            {
                                return Ok(existing.clone());
                            }
                        }
                    }
                    return Err(ProjectionProtocolError::PartitionStopped {
                        failure_id: stopped_failure_id.clone(),
                    });
                }
            }
            if protocol.failures.contains_key(&batch.failure_id) {
                return Err(ProjectionProtocolError::InvalidBatch(format!(
                    "projection failure ID `{}` is already bound to another failure",
                    batch.failure_id
                )));
            }

            protocol.validate_partition(&partition_key, &batch.input, &batch.change_epoch)?;
            match protocol.classify_input(
                &batch.input.cursor,
                batch.input.fingerprint,
                &batch.input.message_id,
                &batch.input.causation_id,
                batch.input.generation,
                batch.input.gap_free,
            )? {
                InputDisposition::New => {
                    protocol.validate_pending_retry(&partition_key, &batch.input)?;
                }
                InputDisposition::Duplicate(_) | InputDisposition::Stale(_) => {
                    return Err(ProjectionProtocolError::InvalidBatch(
                        "cannot record terminal failure for an already processed input".into(),
                    ));
                }
            }

            let receipt = batch.input.inbox_receipt();
            receipt.validate()?;
            if inbox.contains(&(receipt.consumer.clone(), receipt.message_id.clone())) {
                return Err(ProjectionProtocolError::MessageIdReuse {
                    message_id: batch.input.message_id.clone(),
                });
            }

            let mut staged_protocol = protocol.clone();
            let mut staged_inbox = inbox.clone();
            staged_protocol.ensure_partition(&partition_key, &batch.change_epoch)?;
            staged_protocol
                .gap_free_capabilities
                .entry(SourceCapabilityKey::new(&batch.input.cursor))
                .or_insert(batch.input.gap_free);
            let change = staged_protocol.append_change(
                &partition_key,
                PendingChange {
                    kind: ProjectionChangeKind::Failure,
                    causation_id: batch.input.causation_id.clone(),
                    observation_kind: None,
                    scope: None,
                    revision: None,
                    failure_id: Some(batch.failure_id.clone()),
                },
            )?;
            let failure = ProjectionFailure {
                failure_id: batch.failure_id.clone(),
                input: batch.input.cursor.clone(),
                input_fingerprint: batch.input.fingerprint,
                message_id: batch.input.message_id.clone(),
                causation_id: batch.input.causation_id.clone(),
                generation: batch.input.generation,
                gap_free: batch.input.gap_free,
                failure_code: batch.failure_code,
                failure_bytes: batch.failure_bytes,
                failure_digest: batch.failure_digest,
                change: change.cursor,
            };
            staged_protocol
                .partitions
                .get_mut(&partition_key)
                .expect("failure partition was initialized")
                .stopped_failure_id = Some(batch.failure_id.clone());
            staged_protocol
                .partitions
                .get_mut(&partition_key)
                .expect("failure partition was initialized")
                .pending_retry_failure_id = None;
            staged_protocol.persist_input_identity(&batch.input);
            staged_protocol.failure_inputs.insert(
                batch.failure_id.clone(),
                FailedInputFence::from_input(&batch.input),
            );
            staged_protocol
                .failures
                .insert(batch.failure_id, failure.clone());
            staged_protocol
                .retain_change_suffix(&partition_key, self.projection_change_retention)?;
            staged_inbox.insert((receipt.consumer, receipt.message_id));

            *protocol = staged_protocol;
            *inbox = staged_inbox;
            Ok(failure)
        }
    }

    fn projection_checkpoint<'a>(
        &'a self,
        cursor_scope: &'a ProjectionInputCursor,
        generation: ProjectionGeneration,
    ) -> impl Future<Output = Result<Option<ProjectionCheckpoint>, ProjectionProtocolError>> + Send + 'a
    {
        async move {
            let protocol = self
                .projection_protocol
                .read()
                .map_err(|_| RepositoryError::LockPoisoned("projection checkpoint read"))?;
            let Some(input) = protocol
                .inputs
                .get(&InputKey::new(cursor_scope, generation))
            else {
                return Ok(None);
            };
            if matches!(
                cursor_scope.compare_position(&input.cursor),
                RevisionComparison::Incomparable
            ) {
                return Err(ProjectionProtocolError::IncomparableInput);
            }
            Ok(Some(input.checkpoint.clone()))
        }
    }

    fn projection_record<'a>(
        &'a self,
        scope: &'a ProjectionRecordScope,
    ) -> impl Future<Output = Result<Option<ProjectionRecordMetadata>, ProjectionProtocolError>>
           + Send
           + 'a {
        async move {
            let protocol = self
                .projection_protocol
                .read()
                .map_err(|_| RepositoryError::LockPoisoned("projection record read"))?;
            Ok(protocol.records.get(scope).cloned())
        }
    }

    fn projection_input_disposition<'a>(
        &'a self,
        input: &'a TrustedProjectionInput,
    ) -> impl Future<Output = Result<ProjectionInputDisposition, ProjectionProtocolError>> + Send + 'a
    {
        async move {
            let protocol = self
                .projection_protocol
                .read()
                .map_err(|_| RepositoryError::LockPoisoned("projection input disposition read"))?;
            protocol.require_registered_topology(input.cursor.topology())?;
            protocol.validate_known_input_identity(input)?;
            let partition_key = PartitionKey::from_input(&input.cursor);
            let Some(partition) = protocol.partitions.get(&partition_key) else {
                if input.generation != ProjectionGeneration::initial() {
                    return Err(ProjectionProtocolError::GenerationFenced {
                        expected: ProjectionGeneration::initial().get(),
                        actual: input.generation.get(),
                    });
                }
                protocol.validate_gap_free(&input.cursor, input.gap_free)?;
                protocol.validate_input_identity(input)?;
                return Ok(ProjectionInputDisposition::Pending);
            };
            if input.generation != partition.active_generation {
                return Err(ProjectionProtocolError::GenerationFenced {
                    expected: partition.active_generation.get(),
                    actual: input.generation.get(),
                });
            }
            if !protocol.generations.contains_key(&GenerationKey {
                partition: partition_key.clone(),
                generation: input.generation,
            }) {
                return Err(ProjectionProtocolError::InvalidBatch(format!(
                    "active projection generation {} has no durable lineage",
                    input.generation.get()
                )));
            }
            protocol.validate_gap_free(&input.cursor, input.gap_free)?;
            protocol.validate_input_identity(input)?;
            if let Some(failure_id) = &partition.stopped_failure_id {
                return Err(ProjectionProtocolError::PartitionStopped {
                    failure_id: failure_id.clone(),
                });
            }
            match protocol.classify_input(
                &input.cursor,
                input.fingerprint,
                &input.message_id,
                &input.causation_id,
                input.generation,
                input.gap_free,
            )? {
                InputDisposition::Duplicate(checkpoint) => {
                    Ok(ProjectionInputDisposition::Duplicate(checkpoint))
                }
                InputDisposition::Stale(checkpoint) => {
                    Ok(ProjectionInputDisposition::Stale(checkpoint))
                }
                InputDisposition::New => {
                    protocol.validate_pending_retry(&partition_key, input)?;
                    Ok(ProjectionInputDisposition::Pending)
                }
            }
        }
    }

    fn projection_query_snapshot<'a>(
        &'a self,
        request: &'a ProjectionQuerySnapshotRequest,
    ) -> impl Future<Output = Result<ProjectionQuerySnapshot, ProjectionProtocolError>> + Send + 'a
    {
        async move {
            request.validate()?;

            // Projection writers acquire rows before protocol. Holding both
            // read guards in that same order prevents a writer from publishing
            // either half of a row/revision/checkpoint snapshot independently.
            let rows = self
                .model_store
                .relational_rows
                .read()
                .map_err(|_| RepositoryError::LockPoisoned("projection query rows read"))?;
            let protocol = self
                .projection_protocol
                .read()
                .map_err(|_| RepositoryError::LockPoisoned("projection query protocol read"))?;

            read_projection_query_snapshot_from_state(&rows, &protocol, request)
        }
    }

    fn projection_query_snapshot_batch<'a>(
        &'a self,
        request: &'a ProjectionQuerySnapshotBatchRequest,
    ) -> impl Future<Output = Result<ProjectionQuerySnapshotBatch, ProjectionProtocolError>> + Send + 'a
    {
        async move {
            request.validate()?;
            // Use the same writer lock order as the one-row primitive and hold
            // both guards for the entire plan, not once per row.
            let rows =
                self.model_store.relational_rows.read().map_err(|_| {
                    RepositoryError::LockPoisoned("projection query batch rows read")
                })?;
            let protocol = self.projection_protocol.read().map_err(|_| {
                RepositoryError::LockPoisoned("projection query batch protocol read")
            })?;
            let snapshots = request
                .requests
                .iter()
                .map(|row_request| {
                    read_projection_query_snapshot_from_state(&rows, &protocol, row_request)
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ProjectionQuerySnapshotBatch { snapshots })
        }
    }

    fn projection_obligation_evidence_batch<'a>(
        &'a self,
        request: &'a ProjectionObligationEvidenceBatchRequest,
    ) -> impl Future<Output = Result<ProjectionObligationEvidenceBatch, ProjectionProtocolError>>
           + Send
           + 'a {
        async move {
            request.validate()?;
            let protocol = self.projection_protocol.read().map_err(|_| {
                RepositoryError::LockPoisoned("projection obligation evidence read")
            })?;
            let evidence = request
                .requests
                .iter()
                .map(|probe| read_projection_obligation_evidence_from_state(&protocol, probe))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ProjectionObligationEvidenceBatch { evidence })
        }
    }

    fn projection_live_record_batch<'a>(
        &'a self,
        request: &'a ProjectionLiveRecordBatchRequest,
    ) -> impl Future<Output = Result<ProjectionLiveRecordBatch, ProjectionProtocolError>> + Send + 'a
    {
        async move {
            request.validate()?;
            let protocol = self
                .projection_protocol
                .read()
                .map_err(|_| RepositoryError::LockPoisoned("projection live-record read"))?;
            let records = request
                .requests
                .iter()
                .map(|probe| read_projection_live_record_from_state(&protocol, probe))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ProjectionLiveRecordBatch { records })
        }
    }

    fn projection_partition_runtime_state<'a>(
        &'a self,
        topology: &'a ProjectorTopologyId,
        partition: &'a ProjectionPartition,
    ) -> impl Future<Output = Result<Option<ProjectionPartitionRuntimeState>, ProjectionProtocolError>>
           + Send
           + 'a {
        async move {
            let protocol = self.projection_protocol.read().map_err(|_| {
                RepositoryError::LockPoisoned("projection partition runtime state read")
            })?;
            let Some(state) = protocol
                .partitions
                .get(&PartitionKey::new(topology, partition))
            else {
                return Ok(None);
            };
            let pending_retry = match &state.pending_retry_failure_id {
                Some(failure_id) => {
                    let failure = protocol.failures.get(failure_id).ok_or_else(|| {
                        ProjectionProtocolError::InvalidBatch(format!(
                            "pending projection retry failure `{failure_id}` is missing"
                        ))
                    })?;
                    let fence = protocol.failure_inputs.get(failure_id).ok_or_else(|| {
                        ProjectionProtocolError::InvalidBatch(format!(
                            "pending projection retry failure `{failure_id}` has no input fence"
                        ))
                    })?;
                    if state.stopped_failure_id.is_some()
                        || failure.failure_id != *failure_id
                        || failure.input.topology() != topology
                        || failure.input.projection_partition() != partition
                        || failure.generation.checked_next()? != state.active_generation
                        || fence.cursor != failure.input
                        || fence.fingerprint != failure.input_fingerprint
                        || fence.message_id != failure.message_id
                        || fence.causation_id != failure.causation_id
                        || fence.generation != failure.generation
                        || fence.gap_free != failure.gap_free
                    {
                        return Err(ProjectionProtocolError::InvalidBatch(format!(
                            "pending projection retry failure `{failure_id}` is corrupt"
                        )));
                    }
                    Some(ProjectionPendingRetry {
                        failure_id: failure_id.clone(),
                        input: failure.input.clone(),
                        input_fingerprint: failure.input_fingerprint,
                        message_id: failure.message_id.clone(),
                        causation_id: failure.causation_id.clone(),
                        failed_generation: failure.generation,
                        gap_free: failure.gap_free,
                    })
                }
                None => None,
            };
            Ok(Some(ProjectionPartitionRuntimeState {
                active_generation: state.active_generation,
                stopped_failure_id: state.stopped_failure_id.clone(),
                pending_retry,
            }))
        }
    }

    fn projection_observation<'a>(
        &'a self,
        causation_id: &'a str,
        scope: &'a ProjectionRecordScope,
        kind: ProjectionObservationKind,
    ) -> impl Future<Output = Result<Option<ProjectionObservation>, ProjectionProtocolError>> + Send + 'a
    {
        async move {
            let protocol = self
                .projection_protocol
                .read()
                .map_err(|_| RepositoryError::LockPoisoned("projection observation read"))?;
            Ok(protocol
                .observations
                .get(&ObservationKey {
                    causation_id: causation_id.to_string(),
                    scope: scope.clone(),
                    kind,
                })
                .cloned())
        }
    }

    fn projection_changes<'a>(
        &'a self,
        topology: &'a ProjectorTopologyId,
        partition: &'a ProjectionPartition,
        after: Option<&'a ProjectionChangeCursor>,
        limit: usize,
    ) -> impl Future<Output = Result<ProjectionChangeRead, ProjectionProtocolError>> + Send + 'a
    {
        async move {
            let protocol = self
                .projection_protocol
                .read()
                .map_err(|_| RepositoryError::LockPoisoned("projection changes read"))?;
            let key = PartitionKey::new(topology, partition);
            let Some(state) = protocol.partitions.get(&key) else {
                return Ok(match after {
                    Some(_) => ProjectionChangeRead::ResetRequired {
                        head: None,
                        compacted_through: 0,
                    },
                    None => ProjectionChangeRead::Changes {
                        head: None,
                        compacted_through: 0,
                        changes: Vec::new(),
                    },
                });
            };
            let head = if state.change_head == 0 {
                None
            } else {
                Some(ProjectionChangeCursor::new(
                    topology.clone(),
                    partition.clone(),
                    state.change_epoch.clone(),
                    state.change_head,
                )?)
            };
            let after_position = match after {
                None if state.compacted_through > 0 => {
                    return Ok(ProjectionChangeRead::ResetRequired {
                        head,
                        compacted_through: state.compacted_through,
                    });
                }
                None => 0,
                Some(cursor)
                    if cursor.topology() != topology
                        || cursor.projection_partition() != partition
                        || cursor.epoch() != &state.change_epoch
                        || cursor.position() > state.change_head
                        // `compacted_through` is the last removed change. An
                        // exact cursor at that watermark is therefore the safe
                        // boundary for resuming at the first retained change.
                        || cursor.position() < state.compacted_through =>
                {
                    return Ok(ProjectionChangeRead::ResetRequired {
                        head,
                        compacted_through: state.compacted_through,
                    });
                }
                Some(cursor) => cursor.position(),
            };
            let changes = if limit == 0 {
                Vec::new()
            } else {
                state
                    .changes
                    .range((
                        std::ops::Bound::Excluded(after_position),
                        std::ops::Bound::Unbounded,
                    ))
                    .take(limit)
                    .map(|(_, change)| change.clone())
                    .collect()
            };
            Ok(ProjectionChangeRead::Changes {
                head,
                compacted_through: state.compacted_through,
                changes,
            })
        }
    }

    fn repair_projection<'a>(
        &'a self,
        topology: &'a ProjectorTopologyId,
        partition: &'a ProjectionPartition,
        failure_id: &'a str,
    ) -> impl Future<Output = Result<ProjectionGeneration, ProjectionProtocolError>> + Send + 'a
    {
        async move {
            let mut protocol = self
                .projection_protocol
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("projection repair write"))?;
            let key = PartitionKey::new(topology, partition);
            let Some(state) = protocol.partitions.get(&key) else {
                return Err(ProjectionProtocolError::InvalidBatch(
                    "cannot repair an unknown projection partition".into(),
                ));
            };
            match &state.stopped_failure_id {
                Some(stopped) if stopped == failure_id => {}
                Some(stopped) => {
                    return Err(ProjectionProtocolError::PartitionStopped {
                        failure_id: stopped.clone(),
                    });
                }
                None => {
                    return Err(ProjectionProtocolError::InvalidBatch(
                        "projection partition is not stopped".into(),
                    ));
                }
            }
            let Some(failure) = protocol.failures.get(failure_id) else {
                return Err(ProjectionProtocolError::InvalidBatch(format!(
                    "stopped projection failure `{failure_id}` is missing"
                )));
            };
            if failure.input.topology() != topology
                || failure.input.projection_partition() != partition
            {
                return Err(ProjectionProtocolError::ScopeMismatch {
                    field: "projection repair failure",
                });
            }
            if failure.generation != state.active_generation {
                return Err(ProjectionProtocolError::InvalidBatch(format!(
                    "stopped failure generation {} differs from active generation {}",
                    failure.generation.get(),
                    state.active_generation.get()
                )));
            }
            let Some(failure_fence) = protocol.failure_inputs.get(failure_id) else {
                return Err(ProjectionProtocolError::InvalidBatch(format!(
                    "stopped projection failure `{failure_id}` has no exact input fence"
                )));
            };
            if failure_fence.generation != state.active_generation
                || failure_fence.cursor != failure.input
                || failure_fence.fingerprint != failure.input_fingerprint
                || failure_fence.message_id != failure.message_id
                || failure_fence.causation_id != failure.causation_id
            {
                return Err(ProjectionProtocolError::InvalidBatch(format!(
                    "stopped projection failure `{failure_id}` input fence is corrupt"
                )));
            }

            let current_generation = state.active_generation;
            let next_generation = current_generation.checked_next()?;
            if protocol.generations.contains_key(&GenerationKey {
                partition: key.clone(),
                generation: next_generation,
            }) {
                return Err(ProjectionProtocolError::InvalidBatch(format!(
                    "projection repair generation {} already exists",
                    next_generation.get()
                )));
            }
            if protocol
                .generations
                .iter()
                .any(|(generation_key, lineage)| {
                    generation_key.partition == key
                        && lineage.retry_of_failure_id.as_deref() == Some(failure_id)
                })
            {
                return Err(ProjectionProtocolError::InvalidBatch(format!(
                    "stopped failure `{failure_id}` already has a repair generation"
                )));
            }
            let copied_inputs = protocol
                .inputs
                .iter()
                .filter(|(input_key, _)| {
                    input_key.partition == key && input_key.generation == current_generation
                })
                .map(|(input_key, input)| {
                    (
                        InputKey {
                            partition: input_key.partition.clone(),
                            source: input_key.source.clone(),
                            generation: next_generation,
                        },
                        input.clone(),
                    )
                })
                .collect::<Vec<_>>();

            let mut staged = protocol.clone();
            for (input_key, input) in copied_inputs {
                staged.inputs.insert(input_key, input);
            }
            staged.generations.insert(
                GenerationKey {
                    partition: key.clone(),
                    generation: next_generation,
                },
                GenerationLineage::retry_of(current_generation, failure_id.to_string()),
            );
            let state = staged
                .partitions
                .get_mut(&key)
                .expect("repair partition exists in staged state");
            state.active_generation = next_generation;
            state.stopped_failure_id = None;
            state.pending_retry_failure_id = Some(failure_id.to_string());
            *protocol = staged;
            Ok(next_generation)
        }
    }

    fn compact_projection_changes<'a>(
        &'a self,
        through: &'a ProjectionChangeCursor,
    ) -> impl Future<Output = Result<u64, ProjectionProtocolError>> + Send + 'a {
        async move {
            let mut protocol = self
                .projection_protocol
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("projection changes compaction"))?;
            let key = PartitionKey::new(through.topology(), through.projection_partition());
            let Some(partition) = protocol.partitions.get(&key) else {
                return Err(ProjectionProtocolError::InvalidBatch(
                    "cannot compact an unknown projection partition".into(),
                ));
            };
            if through.epoch() != &partition.change_epoch {
                return Err(ProjectionProtocolError::ScopeMismatch {
                    field: "projection compaction epoch",
                });
            }
            if through.position() > partition.change_head {
                return Err(ProjectionProtocolError::InvalidBatch(
                    "cannot compact beyond the projection change head".into(),
                ));
            }
            protocol.compact_changes_through(&key, through.position())
        }
    }

    fn projection_failure<'a>(
        &'a self,
        topology: &'a ProjectorTopologyId,
        partition: &'a ProjectionPartition,
        failure_id: &'a str,
    ) -> impl Future<Output = Result<Option<ProjectionFailure>, ProjectionProtocolError>> + Send + 'a
    {
        async move {
            let protocol = self
                .projection_protocol
                .read()
                .map_err(|_| RepositoryError::LockPoisoned("projection failure read"))?;
            Ok(protocol
                .failures
                .get(failure_id)
                .filter(|failure| {
                    failure.input.topology() == topology
                        && failure.input.projection_partition() == partition
                })
                .cloned())
        }
    }

    fn projection_failure_location<'a>(
        &'a self,
        failure_id: &'a str,
    ) -> impl Future<Output = Result<Option<ProjectionFailureLocation>, ProjectionProtocolError>>
           + Send
           + 'a {
        async move {
            let protocol = self
                .projection_protocol
                .read()
                .map_err(|_| RepositoryError::LockPoisoned("projection failure location read"))?;
            Ok(protocol
                .failures
                .get(failure_id)
                .map(|failure| ProjectionFailureLocation {
                    topology: failure.input.topology().clone(),
                    partition: failure.input.projection_partition().clone(),
                }))
        }
    }
}

fn table_model_name(mutation: &TableMutation) -> &str {
    match mutation {
        TableMutation::UpsertRow(mutation) => &mutation.schema.model_name,
        TableMutation::PatchRow(mutation) => &mutation.schema.model_name,
        TableMutation::DeleteRow(mutation) => &mutation.schema.model_name,
    }
}

fn storage_key_belongs_to_table(storage_key: &str, table: &str) -> bool {
    let Some(mut fingerprint) = storage_key
        .strip_prefix(table)
        .and_then(|suffix| suffix.strip_prefix(':'))
    else {
        return false;
    };
    if fingerprint.is_empty() {
        return false;
    }
    while !fingerprint.is_empty() {
        let Some(length_end) = fingerprint.find(':') else {
            return false;
        };
        let Ok(part_length) = fingerprint[..length_end].parse::<usize>() else {
            return false;
        };
        let part_start = length_end + 1;
        let Some(part_end) = part_start.checked_add(part_length) else {
            return false;
        };
        if fingerprint.as_bytes().get(part_end) != Some(&b';')
            || !fingerprint.is_char_boundary(part_end + 1)
        {
            return false;
        }
        fingerprint = &fingerprint[part_end + 1..];
    }
    true
}

fn checked_next(value: u64, domain: &'static str) -> Result<u64, ProjectionProtocolError> {
    if value >= MAX_PROJECTION_POSITION {
        return Err(ProjectionProtocolError::PositionOverflow { domain });
    }
    Ok(value + 1)
}

fn failure_matches_batch(failure: &ProjectionFailure, batch: &ProjectionFailureBatch) -> bool {
    failure.input == batch.input.cursor
        && failure.input_fingerprint == batch.input.fingerprint
        && failure.message_id == batch.input.message_id
        && failure.causation_id == batch.input.causation_id
        && failure.generation == batch.input.generation
        && failure.failure_code == batch.failure_code
        && failure.failure_bytes == batch.failure_bytes
        && failure.failure_digest == batch.failure_digest
        && failure.change.epoch() == &batch.change_epoch
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier, LazyLock};
    use std::time::Duration;

    use super::*;
    use crate::command_ledger::{
        CanonicalInputHash, CausalCommitBatch, CausalTransactionalCommit,
        CommandContractFingerprint, CommandId, CommandLedgerError, CommandLedgerKey,
        CommandLedgerStore, CommandReservation, PrincipalPartitionId, ReservationOutcome,
        TerminalCommandState,
    };
    use crate::projection_protocol::{
        ProjectionInputDisposition, ProjectionInputFingerprint, ProjectionModelOwnership,
        ProjectionObservationRequest, ProjectionRecordMutation, ProjectionScopeCodec,
        TrustedProjectionInput,
    };
    use crate::repository::{CommitBatch, ReadModelWritePlanStore, TransactionalCommit};
    use crate::table::{
        ColumnType, DeleteTableRowMutation, ExpectedVersion, PatchMode, PatchTableRowMutation,
        PrimaryKey, RowKey, RowPatch, RowValue, RowValues, RowWriteMode, TableColumn, TableKind,
        TableRowMutation, TableSchema,
    };

    fn topology() -> ProjectorTopologyId {
        ProjectorTopologyId::new(1, "todo_projector", [7; 32]).unwrap()
    }

    fn other_topology() -> ProjectorTopologyId {
        ProjectorTopologyId::new(1, "other_projector", [8; 32]).unwrap()
    }

    fn partition() -> ProjectionPartition {
        ProjectionScopeCodec::new(topology())
            .encode_partition(Some(&serde_json::json!("tenant-a")))
            .unwrap()
    }

    fn source() -> ProjectionSource {
        ProjectionSource::new("todo_stream", b"todo-1".to_vec()).unwrap()
    }

    fn input_cursor(position: u64, source_epoch: &str) -> ProjectionInputCursor {
        ProjectionInputCursor::new(
            topology(),
            partition(),
            source(),
            ProjectionEpoch::new(source_epoch).unwrap(),
            position,
        )
        .unwrap()
    }

    fn input(
        position: u64,
        fingerprint: &[u8],
        message_id: &str,
        causation_id: &str,
        generation: ProjectionGeneration,
    ) -> TrustedProjectionInput {
        TrustedProjectionInput::mint(
            input_cursor(position, "source-v1"),
            ProjectionInputFingerprint::from_canonical_bytes(fingerprint),
            message_id,
            causation_id,
            generation,
            true,
        )
        .unwrap()
    }

    fn change_epoch() -> ProjectionEpoch {
        ProjectionEpoch::new("changes-v1").unwrap()
    }

    fn change_cursor(position: u64) -> ProjectionChangeCursor {
        ProjectionChangeCursor::new(topology(), partition(), change_epoch(), position).unwrap()
    }

    fn schema() -> &'static TableSchema {
        static SCHEMA: LazyLock<TableSchema> = LazyLock::new(|| TableSchema {
            model_name: "TodoView".into(),
            table_name: "todo_views".into(),
            columns: vec![
                TableColumn {
                    primary_key: true,
                    ..TableColumn::new("id", "id", ColumnType::Text)
                },
                TableColumn {
                    nullable: true,
                    ..TableColumn::new("value", "value", ColumnType::Text)
                },
            ],
            primary_key: PrimaryKey::new(["id"]),
            version_column: None,
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships: Vec::new(),
            kind: TableKind::ReadModel,
        });
        &SCHEMA
    }

    fn scope_codec() -> ProjectionScopeCodec {
        ProjectionScopeCodec::with_models(topology(), [("TodoView", schema())]).unwrap()
    }

    fn record_key() -> RowKey {
        RowKey::new([("id", RowValue::String("todo-1".into()))])
    }

    fn record_scope() -> ProjectionRecordScope {
        scope_codec()
            .encode_row_scope_in_partition("TodoView", partition(), &record_key())
            .unwrap()
    }

    fn ownership() -> ProjectionModelOwnership {
        ProjectionModelOwnership::new("TodoView", "todo_views").unwrap()
    }

    async fn repository() -> InMemoryRepository {
        repository_with_retention(ProjectionChangeRetention::default()).await
    }

    async fn repository_with_retention(retention: ProjectionChangeRetention) -> InMemoryRepository {
        let repository = InMemoryRepository::new().with_projection_change_retention(retention);
        repository
            .register_projection_models(&topology(), &[ownership()])
            .await
            .unwrap();
        repository
    }

    fn upsert_table_mutation(valid: bool) -> TableMutation {
        let key = record_key();
        let values = if valid {
            let mut values = RowValues::new();
            values.insert("id", RowValue::String("todo-1".into()));
            values
        } else {
            RowValues::new()
        };
        TableMutation::UpsertRow(TableRowMutation {
            schema: schema(),
            key,
            values,
            expected_version: ExpectedVersion::Any,
            mode: RowWriteMode::Upsert,
        })
    }

    fn valued_mutation(
        value: u64,
        expectation: ProjectionRecordExpectation,
    ) -> ProjectionRecordMutation {
        let mut values = RowValues::new();
        values.insert("id", RowValue::String("todo-1".into()));
        values.insert("value", RowValue::String(value.to_string()));
        ProjectionRecordMutation::new(
            record_scope(),
            TableMutation::UpsertRow(TableRowMutation {
                schema: schema(),
                key: record_key(),
                values,
                expected_version: ExpectedVersion::Any,
                mode: RowWriteMode::Upsert,
            }),
            expectation,
            ProjectionMutationKind::Upsert,
        )
        .unwrap()
    }

    fn snapshot_request() -> ProjectionQuerySnapshotRequest {
        ProjectionQuerySnapshotRequest::new(
            &scope_codec(),
            Some(&serde_json::json!("tenant-a")),
            "TodoView",
            record_key(),
            vec![crate::projection_protocol::ProjectionCheckpointProbe::new(
                topology(),
                partition(),
                source(),
                ProjectionEpoch::new("source-v1").unwrap(),
                ProjectionGeneration::initial(),
            )],
        )
        .unwrap()
    }

    fn mutation(
        expectation: ProjectionRecordExpectation,
        kind: ProjectionMutationKind,
    ) -> ProjectionRecordMutation {
        let table = match kind {
            ProjectionMutationKind::Delete => TableMutation::DeleteRow(DeleteTableRowMutation {
                schema: schema(),
                key: RowKey::new([("id", RowValue::String("todo-1".into()))]),
                expected_version: ExpectedVersion::Any,
            }),
            ProjectionMutationKind::Upsert | ProjectionMutationKind::Recreate => {
                upsert_table_mutation(true)
            }
        };
        ProjectionRecordMutation::new(record_scope(), table, expectation, kind).unwrap()
    }

    fn patch_mutation(expected: RecordRevision) -> ProjectionRecordMutation {
        ProjectionRecordMutation::new(
            record_scope(),
            TableMutation::PatchRow(PatchTableRowMutation {
                schema: schema(),
                key: RowKey::new([("id", RowValue::String("todo-1".into()))]),
                patch: RowPatch::new().set("id", RowValue::String("todo-1".into())),
                expected_version: ExpectedVersion::Any,
                mode: PatchMode::UpdateExisting,
            }),
            ProjectionRecordExpectation::Exact(expected),
            ProjectionMutationKind::Upsert,
        )
        .unwrap()
    }

    fn batch(
        trusted: TrustedProjectionInput,
        mutations: Vec<ProjectionRecordMutation>,
        observations: Vec<ProjectionObservationRequest>,
    ) -> ProjectionCommitBatch {
        ProjectionCommitBatch {
            input: trusted,
            change_epoch: change_epoch(),
            ownership: vec![ownership()],
            mutations,
            observations,
        }
    }

    fn row_exists(repository: &InMemoryRepository) -> bool {
        repository
            .model_store
            .relational_rows
            .read()
            .unwrap()
            .contains_key(&upsert_table_mutation(true).lock_key())
    }

    fn row_version(repository: &InMemoryRepository) -> Option<u64> {
        repository
            .model_store
            .relational_rows
            .read()
            .unwrap()
            .get(&upsert_table_mutation(true).lock_key())
            .map(|row| row.version)
    }

    fn direct_batch(causation_id: &str) -> SameTransactionProjectionBatch {
        SameTransactionProjectionBatch::single_upsert(
            topology(),
            partition(),
            change_epoch(),
            ownership(),
            record_scope(),
            upsert_table_mutation(true),
            causation_id,
        )
        .unwrap()
    }

    fn insert_physical_row(repository: &InMemoryRepository) {
        let mut rows = repository.model_store.relational_rows.write().unwrap();
        apply_read_model_write_plan(
            TableWritePlan::new(vec![upsert_table_mutation(true)]),
            &mut rows,
        )
        .unwrap();
    }

    fn remove_physical_row(repository: &InMemoryRepository) {
        repository
            .model_store
            .relational_rows
            .write()
            .unwrap()
            .remove(&upsert_table_mutation(true).lock_key());
    }

    fn assert_query_snapshot_is_coherent(snapshot: &ProjectionQuerySnapshot) {
        let row = snapshot.row.as_ref().expect("physical row");
        let record = snapshot.record.as_ref().expect("record metadata");
        let checkpoint = snapshot.checkpoints[0]
            .checkpoint
            .as_ref()
            .expect("source checkpoint");
        let value = match row.get("value") {
            Some(RowValue::String(value)) => value.parse::<u64>().unwrap(),
            other => panic!("unexpected query snapshot value {other:?}"),
        };
        assert_eq!(value, record.revision.revision());
        assert_eq!(value, checkpoint.input().position());
        assert_eq!(record.change, *checkpoint.change());
        assert_eq!(
            snapshot.change_head.as_ref(),
            Some(checkpoint.change()),
            "live resume head must come from the same snapshot"
        );
        assert_eq!(snapshot.compacted_through, 0);
    }

    #[test]
    fn query_snapshot_probe_and_batch_bounds_are_explicit() {
        let request = snapshot_request();
        assert!(ProjectionQuerySnapshotBatchRequest::new(vec![
            request.clone();
            crate::projection_protocol::MAX_PROJECTION_QUERY_BATCH_ROWS
        ])
        .is_ok());
        assert!(matches!(
            ProjectionQuerySnapshotBatchRequest::new(vec![
                request;
                crate::projection_protocol::MAX_PROJECTION_QUERY_BATCH_ROWS + 1
            ]),
            Err(ProjectionProtocolError::InvalidBatch(message))
                if message.contains("maximum")
        ));

        let probes = (0..crate::projection_protocol::MAX_PROJECTION_QUERY_CHECKPOINT_PROBES)
            .map(|index| {
                crate::projection_protocol::ProjectionCheckpointProbe::new(
                    topology(),
                    partition(),
                    ProjectionSource::new(
                        format!("source-{index}"),
                        format!("partition-{index}").into_bytes(),
                    )
                    .unwrap(),
                    ProjectionEpoch::new("source-v1").unwrap(),
                    ProjectionGeneration::initial(),
                )
            })
            .collect::<Vec<_>>();
        assert!(ProjectionQuerySnapshotRequest::new(
            &scope_codec(),
            Some(&serde_json::json!("tenant-a")),
            "TodoView",
            record_key(),
            probes.clone(),
        )
        .is_ok());
        let max_probe_request = ProjectionQuerySnapshotRequest::new(
            &scope_codec(),
            Some(&serde_json::json!("tenant-a")),
            "TodoView",
            record_key(),
            probes.clone(),
        )
        .unwrap();
        assert!(matches!(
            ProjectionQuerySnapshotBatchRequest::new(vec![max_probe_request; 33]),
            Err(ProjectionProtocolError::InvalidBatch(message))
                if message.contains("aggregate checkpoint probes")
        ));
        let mut over_limit = probes;
        over_limit.push(crate::projection_protocol::ProjectionCheckpointProbe::new(
            topology(),
            partition(),
            ProjectionSource::new("source-over-limit", b"over-limit".to_vec()).unwrap(),
            ProjectionEpoch::new("source-v1").unwrap(),
            ProjectionGeneration::initial(),
        ));
        assert!(matches!(
            ProjectionQuerySnapshotRequest::new(
                &scope_codec(),
                Some(&serde_json::json!("tenant-a")),
                "TodoView",
                record_key(),
                over_limit,
            ),
            Err(ProjectionProtocolError::InvalidBatch(message))
                if message.contains("maximum")
        ));

        let evidence = (0..crate::projection_protocol::MAX_PROJECTION_EVIDENCE_BATCH_ITEMS)
            .map(|index| {
                crate::projection_protocol::ProjectionObligationEvidenceRequest::new(
                    format!("cause-{index}"),
                    record_scope(),
                    ProjectionObservationKind::Record,
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        assert!(
            crate::projection_protocol::ProjectionObligationEvidenceBatchRequest::new(
                evidence.clone()
            )
            .is_ok()
        );
        let mut too_many_evidence = evidence;
        too_many_evidence.push(
            crate::projection_protocol::ProjectionObligationEvidenceRequest::new(
                "cause-over-limit",
                record_scope(),
                ProjectionObservationKind::Record,
            )
            .unwrap(),
        );
        assert!(matches!(
            crate::projection_protocol::ProjectionObligationEvidenceBatchRequest::new(
                too_many_evidence
            ),
            Err(ProjectionProtocolError::InvalidBatch(message))
                if message.contains("maximum")
        ));
        let duplicate = crate::projection_protocol::ProjectionObligationEvidenceRequest::new(
            "duplicate-cause",
            record_scope(),
            ProjectionObservationKind::Record,
        )
        .unwrap();
        assert!(matches!(
            crate::projection_protocol::ProjectionObligationEvidenceBatchRequest::new(vec![
                duplicate.clone(),
                duplicate,
            ]),
            Err(ProjectionProtocolError::InvalidBatch(message))
                if message.contains("repeats")
        ));

        let live = (0..crate::projection_protocol::MAX_PROJECTION_EVIDENCE_BATCH_ITEMS)
            .map(|index| {
                crate::projection_protocol::ProjectionLiveRecordRequest::new(
                    &scope_codec(),
                    "TodoView",
                    RowKey::new([("id", RowValue::String(format!("todo-{index}")))]),
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        assert!(
            crate::projection_protocol::ProjectionLiveRecordBatchRequest::new(live.clone()).is_ok()
        );
        let mut too_many_live = live;
        too_many_live.push(
            crate::projection_protocol::ProjectionLiveRecordRequest::new(
                &scope_codec(),
                "TodoView",
                RowKey::new([("id", RowValue::String("todo-over-limit".into()))]),
            )
            .unwrap(),
        );
        assert!(matches!(
            crate::projection_protocol::ProjectionLiveRecordBatchRequest::new(too_many_live),
            Err(ProjectionProtocolError::InvalidBatch(message))
                if message.contains("maximum")
        ));
    }

    #[tokio::test]
    async fn query_snapshot_never_mixes_row_revision_checkpoint_or_resume_head() {
        let repository = repository().await;
        let first = repository
            .commit_projection(batch(
                input(
                    1,
                    b"snapshot-1",
                    "snapshot-message-1",
                    "snapshot-cause-1",
                    ProjectionGeneration::initial(),
                ),
                vec![valued_mutation(1, ProjectionRecordExpectation::Missing)],
                Vec::new(),
            ))
            .await
            .unwrap();
        let mut expected = first.records[0].revision.clone();
        assert_query_snapshot_is_coherent(
            &repository
                .projection_query_snapshot(&snapshot_request())
                .await
                .unwrap(),
        );

        let writer_repository = repository.clone();
        let writer = tokio::spawn(async move {
            for position in 2..=64 {
                let committed = writer_repository
                    .commit_projection(batch(
                        input(
                            position,
                            format!("snapshot-{position}").as_bytes(),
                            &format!("snapshot-message-{position}"),
                            &format!("snapshot-cause-{position}"),
                            ProjectionGeneration::initial(),
                        ),
                        vec![valued_mutation(
                            position,
                            ProjectionRecordExpectation::Exact(expected),
                        )],
                        Vec::new(),
                    ))
                    .await
                    .unwrap();
                expected = committed.records[0].revision.clone();
                tokio::task::yield_now().await;
            }
        });

        while !writer.is_finished() {
            let batch = repository
                .projection_query_snapshot_batch(
                    &ProjectionQuerySnapshotBatchRequest::new(vec![
                        snapshot_request(),
                        snapshot_request(),
                    ])
                    .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(batch.snapshots[0], batch.snapshots[1]);
            assert_query_snapshot_is_coherent(&batch.snapshots[0]);
            tokio::task::yield_now().await;
        }
        writer.await.unwrap();
        assert_query_snapshot_is_coherent(
            &repository
                .projection_query_snapshot(&snapshot_request())
                .await
                .unwrap(),
        );
    }

    #[tokio::test]
    async fn obligation_evidence_is_exact_bounded_and_failure_wins_after_compaction() {
        let repository = repository().await;
        let scope = record_scope();
        let observed = repository
            .commit_projection(batch(
                input(
                    1,
                    b"observed",
                    "evidence-message-1",
                    "evidence-cause",
                    ProjectionGeneration::initial(),
                ),
                vec![mutation(
                    ProjectionRecordExpectation::Missing,
                    ProjectionMutationKind::Upsert,
                )],
                vec![ProjectionObservationRequest {
                    kind: ProjectionObservationKind::Record,
                    target: ProjectionObservationTarget::StagedRecord(scope.clone()),
                }],
            ))
            .await
            .unwrap();
        let observed_probe = crate::projection_protocol::ProjectionObligationEvidenceRequest::new(
            "evidence-cause",
            scope.clone(),
            ProjectionObservationKind::Record,
        )
        .unwrap();
        let pending_probe = crate::projection_protocol::ProjectionObligationEvidenceRequest::new(
            "pending-cause",
            scope,
            ProjectionObservationKind::Record,
        )
        .unwrap();
        let before_failure = repository
            .projection_obligation_evidence_batch(
                &crate::projection_protocol::ProjectionObligationEvidenceBatchRequest::new(vec![
                    observed_probe.clone(),
                    pending_probe.clone(),
                ])
                .unwrap(),
            )
            .await
            .unwrap();
        assert!(matches!(
            &before_failure.evidence[0],
            ProjectionObligationEvidence::Observed(observation)
                if observation.change == observed.changes[0].cursor
        ));
        assert_eq!(
            before_failure.evidence[1],
            ProjectionObligationEvidence::Pending
        );

        let failure = repository
            .record_projection_failure(
                ProjectionFailureBatch::new(
                    input(
                        2,
                        b"failed",
                        "evidence-message-2",
                        "evidence-cause",
                        ProjectionGeneration::initial(),
                    ),
                    change_epoch(),
                    "evidence-failure",
                    "decode_error",
                    b"bad evidence payload".to_vec(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        repository
            .compact_projection_changes(&failure.change)
            .await
            .unwrap();
        let after_failure = repository
            .projection_obligation_evidence_batch(
                &crate::projection_protocol::ProjectionObligationEvidenceBatchRequest::new(vec![
                    observed_probe,
                    pending_probe,
                ])
                .unwrap(),
            )
            .await
            .unwrap();
        assert!(matches!(
            &after_failure.evidence[0],
            ProjectionObligationEvidence::TerminalFailure(stored)
                if stored == &failure
        ));
        assert_eq!(
            after_failure.evidence[1],
            ProjectionObligationEvidence::Pending
        );
    }

    #[tokio::test]
    async fn unpartitioned_live_record_follows_tombstone_partition_move_and_rejects_ambiguity() {
        let repository = repository().await;
        let old_scope = record_scope();
        let created = repository
            .commit_projection(batch(
                input(
                    1,
                    b"old-partition",
                    "move-message-1",
                    "move-cause-1",
                    ProjectionGeneration::initial(),
                ),
                vec![mutation(
                    ProjectionRecordExpectation::Missing,
                    ProjectionMutationKind::Upsert,
                )],
                Vec::new(),
            ))
            .await
            .unwrap()
            .records
            .remove(0);
        let live_request = crate::projection_protocol::ProjectionLiveRecordBatchRequest::new(vec![
            crate::projection_protocol::ProjectionLiveRecordRequest::new(
                &scope_codec(),
                "TodoView",
                record_key(),
            )
            .unwrap(),
        ])
        .unwrap();
        let live = repository
            .projection_live_record_batch(&live_request)
            .await
            .unwrap();
        assert_eq!(
            live.records[0].as_ref().unwrap().revision.scope(),
            &old_scope
        );

        repository
            .commit_projection(batch(
                input(
                    2,
                    b"old-delete",
                    "move-message-2",
                    "move-cause-2",
                    ProjectionGeneration::initial(),
                ),
                vec![mutation(
                    ProjectionRecordExpectation::Exact(created.revision),
                    ProjectionMutationKind::Delete,
                )],
                Vec::new(),
            ))
            .await
            .unwrap();
        assert_eq!(
            repository
                .projection_live_record_batch(&live_request)
                .await
                .unwrap()
                .records,
            vec![None]
        );

        let new_partition = scope_codec()
            .encode_partition(Some(&serde_json::json!("tenant-b")))
            .unwrap();
        let new_scope = scope_codec()
            .encode_row_scope_in_partition("TodoView", new_partition.clone(), &record_key())
            .unwrap();
        let new_input = TrustedProjectionInput::mint(
            ProjectionInputCursor::new(
                topology(),
                new_partition,
                source(),
                ProjectionEpoch::new("source-v1").unwrap(),
                1,
            )
            .unwrap(),
            ProjectionInputFingerprint::from_canonical_bytes(b"new-partition"),
            "move-message-3",
            "move-cause-3",
            ProjectionGeneration::initial(),
            true,
        )
        .unwrap();
        let moved = repository
            .commit_projection(batch(
                new_input,
                vec![ProjectionRecordMutation::new(
                    new_scope.clone(),
                    upsert_table_mutation(true),
                    ProjectionRecordExpectation::Missing,
                    ProjectionMutationKind::Upsert,
                )
                .unwrap()],
                Vec::new(),
            ))
            .await
            .unwrap()
            .records
            .remove(0);
        assert_eq!(
            repository
                .projection_live_record_batch(&live_request)
                .await
                .unwrap()
                .records[0]
                .as_ref()
                .unwrap()
                .revision
                .scope(),
            &new_scope
        );
        assert_eq!(moved.revision.scope(), &new_scope);

        // Simulate durable metadata drift that bypassed the normal write
        // fence. The read path must report corruption rather than choosing.
        repository
            .projection_protocol
            .write()
            .unwrap()
            .records
            .get_mut(&old_scope)
            .unwrap()
            .tombstone = false;
        assert!(matches!(
            repository.projection_live_record_batch(&live_request).await,
            Err(ProjectionProtocolError::InvalidBatch(message))
                if message.contains("ambiguous")
        ));
    }

    fn other_source_input(
        message_id: &str,
        generation: ProjectionGeneration,
    ) -> TrustedProjectionInput {
        TrustedProjectionInput::mint(
            ProjectionInputCursor::new(
                topology(),
                partition(),
                ProjectionSource::new("other_stream", b"other-1".to_vec()).unwrap(),
                ProjectionEpoch::new("source-v1").unwrap(),
                0,
            )
            .unwrap(),
            ProjectionInputFingerprint::from_canonical_bytes(b"other-input"),
            message_id,
            "other-cause",
            generation,
            true,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn direct_projection_is_fenced_while_exact_failure_repair_is_pending() {
        let repository = repository().await;
        assert_eq!(
            repository
                .projection_partition_runtime_state(&topology(), &partition())
                .await
                .unwrap(),
            None
        );
        let failure = repository
            .record_projection_failure(
                ProjectionFailureBatch::new(
                    input(
                        0,
                        b"failed",
                        "failed-message",
                        "failed-cause",
                        ProjectionGeneration::initial(),
                    ),
                    change_epoch(),
                    "failure-1",
                    "decode_error",
                    b"bad payload".to_vec(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert!(failure.gap_free);
        let stopped = repository
            .projection_partition_runtime_state(&topology(), &partition())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stopped.active_generation, ProjectionGeneration::initial());
        assert_eq!(stopped.stopped_failure_id.as_deref(), Some("failure-1"));
        assert_eq!(stopped.pending_retry, None);
        repository
            .repair_projection(&topology(), &partition(), "failure-1")
            .await
            .unwrap();
        let repaired = repository
            .projection_partition_runtime_state(&topology(), &partition())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(repaired.active_generation.get(), 2);
        assert_eq!(repaired.stopped_failure_id, None);
        let retry = repaired.pending_retry.unwrap();
        assert_eq!(retry.failure_id, "failure-1");
        assert_eq!(retry.input, failure.input);
        assert_eq!(retry.input_fingerprint, failure.input_fingerprint);
        assert_eq!(retry.message_id, failure.message_id);
        assert_eq!(retry.causation_id, failure.causation_id);
        assert_eq!(retry.failed_generation, failure.generation);
        assert_eq!(retry.gap_free, failure.gap_free);

        let mut staged_protocol = repository.projection_protocol.read().unwrap().clone();
        let mut staged_rows = repository
            .model_store
            .relational_rows
            .read()
            .unwrap()
            .clone();
        assert!(matches!(
            stage_same_transaction_projection(
                &mut staged_protocol,
                &mut staged_rows,
                &direct_batch("direct-during-repair"),
                repository.projection_change_retention,
            ),
            Err(ProjectionProtocolError::IncomparableInput)
        ));

        let partition_key = PartitionKey::new(&topology(), &partition());
        let partition = staged_protocol.partitions.get(&partition_key).unwrap();
        assert_eq!(partition.change_head, 1);
        assert_eq!(partition.compacted_through, 0);
        assert_eq!(
            partition.pending_retry_failure_id.as_deref(),
            Some("failure-1")
        );
        assert!(staged_protocol.ownership.is_empty());
        assert!(staged_protocol.records.is_empty());
        assert!(staged_protocol.observations.is_empty());
        assert!(staged_rows.is_empty());
    }

    #[tokio::test]
    async fn direct_projection_reports_typed_physical_metadata_drift() {
        let orphan_row = repository().await;
        insert_physical_row(&orphan_row);
        let mut staged_protocol = orphan_row.projection_protocol.read().unwrap().clone();
        let mut staged_rows = orphan_row
            .model_store
            .relational_rows
            .read()
            .unwrap()
            .clone();
        assert!(matches!(
            stage_same_transaction_projection(
                &mut staged_protocol,
                &mut staged_rows,
                &direct_batch("direct-orphan-row"),
                orphan_row.projection_change_retention,
            ),
            Err(ProjectionProtocolError::RecordAlreadyExists { model })
                if model == "TodoView"
        ));
        assert!(orphan_row
            .projection_record(&record_scope())
            .await
            .unwrap()
            .is_none());
        assert!(row_exists(&orphan_row));

        let missing_row = repository().await;
        let metadata = missing_row
            .commit_projection(batch(
                input(
                    0,
                    b"create",
                    "create-message",
                    "create-cause",
                    ProjectionGeneration::initial(),
                ),
                vec![mutation(
                    ProjectionRecordExpectation::Missing,
                    ProjectionMutationKind::Upsert,
                )],
                Vec::new(),
            ))
            .await
            .unwrap()
            .records
            .remove(0);
        remove_physical_row(&missing_row);
        let mut staged_protocol = missing_row.projection_protocol.read().unwrap().clone();
        let mut staged_rows = missing_row
            .model_store
            .relational_rows
            .read()
            .unwrap()
            .clone();
        assert!(matches!(
            stage_same_transaction_projection(
                &mut staged_protocol,
                &mut staged_rows,
                &direct_batch("direct-missing-row"),
                missing_row.projection_change_retention,
            ),
            Err(ProjectionProtocolError::RecordMissing { model })
                if model == "TodoView"
        ));
        assert_eq!(
            missing_row
                .projection_record(&record_scope())
                .await
                .unwrap(),
            Some(metadata)
        );
        assert!(!row_exists(&missing_row));
    }

    #[tokio::test]
    async fn stale_direct_attempt_is_fenced_before_projection_drift_is_inspected() {
        let repository = repository().await;
        let retention = Duration::from_secs(3_600);
        let command_key = CommandLedgerKey::new(
            "in-memory-direct-precedence",
            PrincipalPartitionId::new("tenant:direct-precedence").unwrap(),
            CommandId::parse(uuid::Uuid::now_v7().hyphenated().to_string()).unwrap(),
        )
        .unwrap();
        let reservation = CommandReservation::new(
            command_key,
            "project-todo",
            CommandContractFingerprint::new([41; 32]),
            CanonicalInputHash::new([42; 32]),
            Duration::from_secs(30),
            retention,
        )
        .unwrap();
        let attempt = match repository.reserve_command(reservation).await.unwrap() {
            ReservationOutcome::Acquired(attempt) => attempt,
            _ => panic!("a fresh direct command must acquire its first attempt"),
        };
        let direct = direct_batch(attempt.causation_id().as_str());
        let completion = attempt
            .complete(
                TerminalCommandState::Projected,
                serde_json::json!({"projected": "must-not-run"}),
                retention,
            )
            .unwrap();
        repository
            .mark_retryable_unknown(completion.attempt_fence())
            .await
            .unwrap();

        // This orphan physical row would produce RecordAlreadyExists if the
        // direct projection inspected protocol state before the stale attempt.
        insert_physical_row(&repository);
        assert!(matches!(
            repository
                .commit_causal_batch(CausalCommitBatch::with_direct_projection(
                    CommitBatch::empty(),
                    completion,
                    direct,
                ))
                .await,
            Err(CommandLedgerError::AttemptFenced { .. })
        ));
        assert!(row_exists(&repository));
        assert!(repository
            .projection_record(&record_scope())
            .await
            .unwrap()
            .is_none());
        assert!(repository
            .projection_protocol
            .read()
            .unwrap()
            .partitions
            .is_empty());
    }

    #[tokio::test]
    async fn automatic_change_retention_compacts_success_and_failure_prefixes() {
        let repository =
            repository_with_retention(ProjectionChangeRetention::new(2).unwrap()).await;
        for position in 0..4 {
            repository
                .commit_projection(batch(
                    input(
                        position,
                        format!("input-{position}").as_bytes(),
                        &format!("message-{position}"),
                        &format!("cause-{position}"),
                        ProjectionGeneration::initial(),
                    ),
                    Vec::new(),
                    Vec::new(),
                ))
                .await
                .unwrap();
        }

        assert_eq!(
            repository
                .projection_changes(&topology(), &partition(), None, 100)
                .await
                .unwrap(),
            ProjectionChangeRead::ResetRequired {
                head: Some(change_cursor(4)),
                compacted_through: 2,
            }
        );
        let retained = repository
            .projection_changes(&topology(), &partition(), Some(&change_cursor(2)), 100)
            .await
            .unwrap();
        let ProjectionChangeRead::Changes {
            head,
            compacted_through,
            changes,
        } = retained
        else {
            panic!("the exact compacted-through cursor must resume retained changes");
        };
        assert_eq!(head, Some(change_cursor(4)));
        assert_eq!(compacted_through, 2);
        assert_eq!(
            changes
                .iter()
                .map(|change| change.cursor.position())
                .collect::<Vec<_>>(),
            vec![3, 4]
        );
        assert!(matches!(
            repository
                .projection_changes(&topology(), &partition(), Some(&change_cursor(1)), 100)
                .await
                .unwrap(),
            ProjectionChangeRead::ResetRequired {
                compacted_through: 2,
                ..
            }
        ));

        repository
            .record_projection_failure(
                ProjectionFailureBatch::new(
                    input(
                        4,
                        b"failure",
                        "message-4",
                        "cause-4",
                        ProjectionGeneration::initial(),
                    ),
                    change_epoch(),
                    "failure-4",
                    "decode_error",
                    b"bad payload".to_vec(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let retained = repository
            .projection_changes(&topology(), &partition(), Some(&change_cursor(3)), 100)
            .await
            .unwrap();
        let ProjectionChangeRead::Changes {
            head,
            compacted_through,
            changes,
        } = retained
        else {
            panic!("the new compacted-through cursor must resume retained changes");
        };
        assert_eq!(head, Some(change_cursor(5)));
        assert_eq!(compacted_through, 3);
        assert_eq!(
            changes
                .iter()
                .map(|change| (change.cursor.position(), change.kind))
                .collect::<Vec<_>>(),
            vec![
                (4, ProjectionChangeKind::Checkpoint),
                (5, ProjectionChangeKind::Failure),
            ]
        );
    }

    #[tokio::test]
    async fn direct_projection_retention_preserves_durable_evidence() {
        let repository =
            repository_with_retention(ProjectionChangeRetention::new(1).unwrap()).await;
        let mut staged_protocol = repository.projection_protocol.read().unwrap().clone();
        let mut staged_rows = HashMap::new();

        let first = stage_same_transaction_projection(
            &mut staged_protocol,
            &mut staged_rows,
            &direct_batch("direct-1"),
            repository.projection_change_retention,
        )
        .unwrap();
        let second = stage_same_transaction_projection(
            &mut staged_protocol,
            &mut staged_rows,
            &direct_batch("direct-2"),
            repository.projection_change_retention,
        )
        .unwrap();

        let partition = staged_protocol
            .partitions
            .get(&PartitionKey::new(&topology(), &partition()))
            .unwrap();
        assert_eq!(partition.change_head, 2);
        assert_eq!(partition.compacted_through, 1);
        assert_eq!(
            partition.changes.keys().copied().collect::<Vec<_>>(),
            vec![2]
        );
        assert_eq!(first.changes[0].cursor, change_cursor(1));
        assert_eq!(second.changes[0].cursor, change_cursor(2));
        assert_eq!(
            staged_protocol
                .records
                .get(&record_scope())
                .unwrap()
                .revision
                .revision(),
            2
        );
        assert!(staged_protocol.observations.contains_key(&ObservationKey {
            causation_id: "direct-1".into(),
            scope: record_scope(),
            kind: ProjectionObservationKind::Record,
        }));
        assert!(staged_protocol.observations.contains_key(&ObservationKey {
            causation_id: "direct-2".into(),
            scope: record_scope(),
            kind: ProjectionObservationKind::Record,
        }));
        assert!(staged_rows.contains_key(&upsert_table_mutation(true).lock_key()));
    }

    #[tokio::test]
    async fn automatic_retention_failure_rolls_back_rows_protocol_and_inbox() {
        let repository =
            repository_with_retention(ProjectionChangeRetention::new(1).unwrap()).await;
        let created = repository
            .commit_projection(batch(
                input(
                    0,
                    b"create",
                    "message-0",
                    "cause-0",
                    ProjectionGeneration::initial(),
                ),
                vec![mutation(
                    ProjectionRecordExpectation::Missing,
                    ProjectionMutationKind::Upsert,
                )],
                Vec::new(),
            ))
            .await
            .unwrap()
            .records
            .remove(0);
        let row_version_before = row_version(&repository);
        let inbox_before = repository.inbox_store.read().unwrap().clone();
        {
            let mut protocol = repository.projection_protocol.write().unwrap();
            protocol
                .partitions
                .get_mut(&PartitionKey::new(&topology(), &partition()))
                .unwrap()
                .changes
                .remove(&1);
        }

        assert!(matches!(
            repository
                .commit_projection(batch(
                    input(
                        1,
                        b"update",
                        "message-1",
                        "cause-1",
                        ProjectionGeneration::initial(),
                    ),
                    vec![mutation(
                        ProjectionRecordExpectation::Exact(created.revision.clone()),
                        ProjectionMutationKind::Upsert,
                    )],
                    Vec::new(),
                ))
                .await,
            Err(ProjectionProtocolError::InvalidBatch(message))
                if message.contains(
                    "projection change compaction expected to remove 1 contiguous entries but found 0"
                )
        ));

        assert_eq!(row_version(&repository), row_version_before);
        assert_eq!(
            repository.projection_record(&record_scope()).await.unwrap(),
            Some(created)
        );
        assert_eq!(*repository.inbox_store.read().unwrap(), inbox_before);
        let protocol = repository.projection_protocol.read().unwrap();
        let partition = protocol
            .partitions
            .get(&PartitionKey::new(&topology(), &partition()))
            .unwrap();
        assert_eq!(partition.change_head, 1);
        assert_eq!(partition.compacted_through, 0);
        assert!(partition.changes.is_empty());
        assert!(!protocol
            .input_identities
            .contains_key(&CursorIdentityKey::new(&input_cursor(1, "source-v1"))));
        assert!(!protocol
            .applied_receipts
            .contains_key(&CursorReceiptKey::new(
                &input_cursor(1, "source-v1"),
                ProjectionGeneration::initial(),
            )));
    }

    #[tokio::test]
    async fn lengthening_retention_never_restores_a_compacted_prefix() {
        let repository =
            repository_with_retention(ProjectionChangeRetention::new(1).unwrap()).await;
        for position in 0..3 {
            repository
                .commit_projection(batch(
                    input(
                        position,
                        format!("input-{position}").as_bytes(),
                        &format!("message-{position}"),
                        &format!("cause-{position}"),
                        ProjectionGeneration::initial(),
                    ),
                    Vec::new(),
                    Vec::new(),
                ))
                .await
                .unwrap();
        }
        assert_eq!(
            repository
                .compact_projection_changes(&change_cursor(3))
                .await
                .unwrap(),
            3
        );

        let lengthened = repository
            .clone()
            .with_projection_change_retention(ProjectionChangeRetention::new(10).unwrap());
        lengthened
            .commit_projection(batch(
                input(
                    3,
                    b"input-3",
                    "message-3",
                    "cause-3",
                    ProjectionGeneration::initial(),
                ),
                Vec::new(),
                Vec::new(),
            ))
            .await
            .unwrap();
        assert_eq!(
            lengthened
                .projection_changes(&topology(), &partition(), None, 100)
                .await
                .unwrap(),
            ProjectionChangeRead::ResetRequired {
                head: Some(change_cursor(4)),
                compacted_through: 3,
            }
        );
        assert_eq!(
            lengthened
                .projection_changes(&topology(), &partition(), Some(&change_cursor(3)), 100)
                .await
                .unwrap(),
            ProjectionChangeRead::Changes {
                head: Some(change_cursor(4)),
                compacted_through: 3,
                changes: vec![ProjectionChange {
                    cursor: change_cursor(4),
                    kind: ProjectionChangeKind::Checkpoint,
                    causation_id: "cause-3".into(),
                    observation_kind: None,
                    scope: None,
                    revision: None,
                    failure_id: None,
                }],
            }
        );
    }

    #[tokio::test]
    async fn malformed_projection_failure_is_rejected_before_memory_writes() {
        let repository = repository().await;
        let mut failure = ProjectionFailureBatch::new(
            input(
                0,
                b"malformed-failure",
                "message-malformed-failure",
                "cause-malformed-failure",
                ProjectionGeneration::initial(),
            ),
            change_epoch(),
            "failure-malformed",
            "decode_error",
            b"bad payload".to_vec(),
        )
        .unwrap();
        failure.failure_digest = [0; 32];

        assert!(matches!(
            repository.record_projection_failure(failure).await,
            Err(ProjectionProtocolError::InvalidBatch(message))
                if message.contains("digest does not match")
        ));
        assert!(repository
            .projection_failure(&topology(), &partition(), "failure-malformed")
            .await
            .unwrap()
            .is_none());
        assert!(repository
            .projection_partition_runtime_state(&topology(), &partition())
            .await
            .unwrap()
            .is_none());
        assert!(repository.inbox_store.read().unwrap().is_empty());
    }

    #[tokio::test]
    async fn topology_bootstrap_is_required_for_noop_success_and_failure() {
        let repository = InMemoryRepository::new();
        let noop = ProjectionCommitBatch {
            input: input(
                0,
                b"noop",
                "message-noop",
                "cause-noop",
                ProjectionGeneration::initial(),
            ),
            change_epoch: change_epoch(),
            ownership: Vec::new(),
            mutations: Vec::new(),
            observations: Vec::new(),
        };
        assert!(matches!(
            repository.commit_projection(noop).await,
            Err(ProjectionProtocolError::InvalidBatch(message))
                if message.contains("not bootstrapped")
        ));
        let failure = ProjectionFailureBatch::new(
            input(
                0,
                b"failure",
                "message-failure",
                "cause-failure",
                ProjectionGeneration::initial(),
            ),
            change_epoch(),
            "failure-unbootstrapped",
            "decode_error",
            b"bad payload".to_vec(),
        )
        .unwrap();
        assert!(matches!(
            repository.record_projection_failure(failure).await,
            Err(ProjectionProtocolError::InvalidBatch(message))
                if message.contains("not bootstrapped")
        ));
        assert!(repository
            .projection_protocol
            .read()
            .unwrap()
            .partitions
            .is_empty());
        assert!(repository.inbox_store.read().unwrap().is_empty());

        assert!(matches!(
            repository.register_projection_models(&topology(), &[]).await,
            Err(ProjectionProtocolError::InvalidBatch(message))
                if message.contains("at least one model/table owner")
        ));
        repository
            .register_projection_models(&topology(), &[ownership()])
            .await
            .unwrap();
        let applied = repository
            .commit_projection(batch(
                input(
                    0,
                    b"noop",
                    "message-noop",
                    "cause-noop",
                    ProjectionGeneration::initial(),
                ),
                Vec::new(),
                Vec::new(),
            ))
            .await
            .unwrap();
        assert_eq!(applied.outcome, ProjectionCommitOutcome::Applied);
        repository
            .record_projection_failure(
                ProjectionFailureBatch::new(
                    input(
                        1,
                        b"failure",
                        "message-failure",
                        "cause-failure",
                        ProjectionGeneration::initial(),
                    ),
                    change_epoch(),
                    "failure-bootstrapped",
                    "decode_error",
                    b"bad payload".to_vec(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn registration_is_global_and_rejects_unowned_rows_atomically() {
        let repository = InMemoryRepository::new();
        repository
            .model_store()
            .commit_write_plan(TableWritePlan::new(vec![upsert_table_mutation(true)]))
            .await
            .unwrap();
        assert!(matches!(
            repository
                .register_projection_models(&topology(), &[ownership()])
                .await,
            Err(ProjectionProtocolError::InvalidBatch(message))
                if message.contains("contains rows without causal metadata")
        ));
        {
            let protocol = repository.projection_protocol.read().unwrap();
            assert!(!protocol.registered_topologies.contains(&topology()));
            assert!(protocol.registered_models.is_empty());
            assert!(protocol.authoritative_table_owners.is_empty());
        }
        assert!(!repository
            .causal_tables
            .read()
            .unwrap()
            .contains("todo_views"));
        repository
            .model_store()
            .commit_write_plan(TableWritePlan::new(vec![upsert_table_mutation(true)]))
            .await
            .unwrap();

        let clean = InMemoryRepository::new();
        clean
            .register_projection_models(&topology(), &[ownership()])
            .await
            .unwrap();
        assert!(matches!(
            clean
                .register_projection_models(&other_topology(), &[ownership()])
                .await,
            Err(ProjectionProtocolError::InvalidBatch(message))
                if message.contains("authoritative owner")
        ));
        let protocol = clean.projection_protocol.read().unwrap();
        assert!(!protocol.registered_topologies.contains(&other_topology()));
        assert_eq!(
            protocol
                .authoritative_table_owners
                .get("todo_views")
                .map(|owner| (&owner.topology, owner.model.as_str())),
            Some((&topology(), "TodoView"))
        );
    }

    #[tokio::test]
    async fn cursor_fences_are_exact_and_non_mutating() {
        let repository = repository().await;
        let applied = repository
            .commit_projection(batch(
                input(
                    1,
                    b"input-1",
                    "message-1",
                    "cause-1",
                    ProjectionGeneration::initial(),
                ),
                vec![mutation(
                    ProjectionRecordExpectation::Missing,
                    ProjectionMutationKind::Upsert,
                )],
                Vec::new(),
            ))
            .await
            .unwrap();
        assert_eq!(applied.outcome, ProjectionCommitOutcome::Applied);
        assert!(row_exists(&repository));

        let duplicate = repository
            .commit_projection(batch(
                input(
                    1,
                    b"input-1",
                    "message-1",
                    "cause-1",
                    ProjectionGeneration::initial(),
                ),
                vec![mutation(
                    ProjectionRecordExpectation::Missing,
                    ProjectionMutationKind::Upsert,
                )],
                Vec::new(),
            ))
            .await
            .unwrap();
        assert_eq!(duplicate.outcome, ProjectionCommitOutcome::Duplicate);
        assert!(duplicate.changes.is_empty());

        for corrupted in [
            input(
                1,
                b"input-1",
                "different-message",
                "cause-1",
                ProjectionGeneration::initial(),
            ),
            input(
                1,
                b"input-1",
                "message-1",
                "different-cause",
                ProjectionGeneration::initial(),
            ),
        ] {
            assert!(matches!(
                repository
                    .commit_projection(batch(corrupted, Vec::new(), Vec::new()))
                    .await,
                Err(ProjectionProtocolError::InputCorruption)
            ));
        }

        let stale = repository
            .commit_projection(batch(
                input(
                    0,
                    b"older",
                    "message-0",
                    "cause-0",
                    ProjectionGeneration::initial(),
                ),
                Vec::new(),
                Vec::new(),
            ))
            .await
            .unwrap();
        assert_eq!(stale.outcome, ProjectionCommitOutcome::StaleInput);
        assert!(matches!(
            repository
                .commit_projection(batch(
                    input(
                        0,
                        b"older",
                        "message-1",
                        "cause-0",
                        ProjectionGeneration::initial(),
                    ),
                    Vec::new(),
                    Vec::new(),
                ))
                .await,
            Err(ProjectionProtocolError::MessageIdReuse { .. })
        ));
        let incomparable = TrustedProjectionInput::mint(
            input_cursor(2, "source-v2"),
            ProjectionInputFingerprint::from_canonical_bytes(b"new-epoch"),
            "message-new-epoch",
            "cause-new-epoch",
            ProjectionGeneration::initial(),
            true,
        )
        .unwrap();
        assert!(matches!(
            repository
                .commit_projection(batch(incomparable, Vec::new(), Vec::new()))
                .await,
            Err(ProjectionProtocolError::IncomparableInput)
        ));

        assert!(matches!(
            repository
                .commit_projection(batch(
                    input(
                        1,
                        b"corrupt",
                        "message-corrupt",
                        "cause-1",
                        ProjectionGeneration::initial(),
                    ),
                    Vec::new(),
                    Vec::new(),
                ))
                .await,
            Err(ProjectionProtocolError::InputCorruption)
        ));
        assert_eq!(
            repository
                .projection_changes(&topology(), &partition(), None, 100)
                .await
                .unwrap(),
            ProjectionChangeRead::Changes {
                head: applied
                    .checkpoint
                    .map(|checkpoint| checkpoint.change().clone()),
                compacted_through: 0,
                changes: applied.changes,
            }
        );
    }

    #[tokio::test]
    async fn message_receipts_and_gap_capability_are_immutable() {
        let repo = repository().await;
        for (position, fingerprint, message, cause) in [
            (1, b"one".as_slice(), "message-1", "cause-1"),
            (2, b"two".as_slice(), "message-2", "cause-2"),
        ] {
            repo.commit_projection(batch(
                input(
                    position,
                    fingerprint,
                    message,
                    cause,
                    ProjectionGeneration::initial(),
                ),
                Vec::new(),
                Vec::new(),
            ))
            .await
            .unwrap();
        }

        let old_duplicate = repo
            .commit_projection(batch(
                input(
                    1,
                    b"one",
                    "message-1",
                    "cause-1",
                    ProjectionGeneration::initial(),
                ),
                Vec::new(),
                Vec::new(),
            ))
            .await
            .unwrap();
        assert_eq!(old_duplicate.outcome, ProjectionCommitOutcome::Duplicate);

        let changed_old_cursor = input(
            1,
            b"one",
            "new-message-at-old-cursor",
            "cause-1",
            ProjectionGeneration::initial(),
        );
        assert!(matches!(
            repo.commit_projection(batch(changed_old_cursor, Vec::new(), Vec::new()))
                .await,
            Err(ProjectionProtocolError::InputCorruption)
        ));

        let reused_old_message = input(
            3,
            b"three",
            "message-1",
            "cause-3",
            ProjectionGeneration::initial(),
        );
        assert!(matches!(
            repo.commit_projection(batch(reused_old_message, Vec::new(), Vec::new()))
                .await,
            Err(ProjectionProtocolError::MessageIdReuse { .. })
        ));

        assert!(matches!(
            repo.commit_projection(batch(
                input(
                    4,
                    b"gap",
                    "gap-message",
                    "gap-cause",
                    ProjectionGeneration::initial(),
                ),
                Vec::new(),
                Vec::new(),
            ))
            .await,
            Err(ProjectionProtocolError::IncomparableInput)
        ));

        let changed_causation = TrustedProjectionInput::mint(
            input_cursor(1, "source-v1"),
            ProjectionInputFingerprint::from_canonical_bytes(b"one"),
            "message-1",
            "changed-cause",
            ProjectionGeneration::initial(),
            true,
        )
        .unwrap();
        assert!(matches!(
            repo.commit_projection(batch(changed_causation, Vec::new(), Vec::new()))
                .await,
            Err(ProjectionProtocolError::InputCorruption)
        ));

        for (position, fingerprint, message, cause) in [
            (1, b"one".as_slice(), "old-capability", "cause-1"),
            (2, b"two".as_slice(), "equal-capability", "cause-2"),
            (3, b"three".as_slice(), "new-capability", "cause-3"),
        ] {
            let changed_capability = TrustedProjectionInput::mint(
                input_cursor(position, "source-v1"),
                ProjectionInputFingerprint::from_canonical_bytes(fingerprint),
                message,
                cause,
                ProjectionGeneration::initial(),
                false,
            )
            .unwrap();
            assert!(matches!(
                repo.commit_projection(batch(changed_capability, Vec::new(), Vec::new()))
                    .await,
                Err(ProjectionProtocolError::InputCorruption)
            ));
        }

        // A first-ever terminal failure also registers the source capability;
        // repair generations cannot silently redefine it merely because there
        // was no last-good checkpoint to copy.
        let failed_first = repository().await;
        failed_first
            .record_projection_failure(
                ProjectionFailureBatch::new(
                    input(
                        0,
                        b"failed-first",
                        "failed-first-message",
                        "failed-first-cause",
                        ProjectionGeneration::initial(),
                    ),
                    change_epoch(),
                    "failed-first-id",
                    "decode_error",
                    b"bad first payload".to_vec(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let repaired_generation = failed_first
            .repair_projection(&topology(), &partition(), "failed-first-id")
            .await
            .unwrap();
        let changed_after_repair = TrustedProjectionInput::mint(
            input_cursor(0, "source-v1"),
            ProjectionInputFingerprint::from_canonical_bytes(b"retry"),
            "retry-message",
            "retry-cause",
            repaired_generation,
            false,
        )
        .unwrap();
        assert!(matches!(
            failed_first
                .commit_projection(batch(changed_after_repair, Vec::new(), Vec::new()))
                .await,
            Err(ProjectionProtocolError::InputCorruption)
        ));
    }

    #[tokio::test]
    async fn message_identity_is_topology_wide_across_projection_partitions() {
        let repository = repository().await;
        repository
            .commit_projection(batch(
                input(
                    1,
                    b"topology-wide-message",
                    "topology-wide-message",
                    "topology-wide-cause",
                    ProjectionGeneration::initial(),
                ),
                Vec::new(),
                Vec::new(),
            ))
            .await
            .unwrap();

        let other_partition = ProjectionScopeCodec::new(topology())
            .encode_partition(Some(&serde_json::json!("tenant-b")))
            .unwrap();
        let remapped = TrustedProjectionInput::mint(
            ProjectionInputCursor::new(
                topology(),
                other_partition,
                source(),
                ProjectionEpoch::new("source-v1").unwrap(),
                1,
            )
            .unwrap(),
            ProjectionInputFingerprint::from_canonical_bytes(b"topology-wide-message"),
            "topology-wide-message",
            "topology-wide-cause",
            ProjectionGeneration::initial(),
            true,
        )
        .unwrap();

        assert!(matches!(
            repository
                .commit_projection(batch(remapped, Vec::new(), Vec::new()))
                .await,
            Err(ProjectionProtocolError::MessageIdReuse { message_id })
                if message_id == "topology-wide-message"
        ));
    }

    #[tokio::test]
    async fn input_disposition_is_read_only_exact_and_repair_fenced() {
        let repository = repository().await;
        let first_input = input(
            1,
            b"preflight-one",
            "preflight-message-1",
            "preflight-cause-1",
            ProjectionGeneration::initial(),
        );
        assert_eq!(
            repository
                .projection_input_disposition(&first_input)
                .await
                .unwrap(),
            ProjectionInputDisposition::Pending
        );
        assert_eq!(
            repository
                .projection_partition_runtime_state(&topology(), &partition())
                .await
                .unwrap(),
            None,
            "a preflight read must not create protocol state"
        );

        let applied = repository
            .commit_projection(batch(first_input.clone(), Vec::new(), Vec::new()))
            .await
            .unwrap();
        assert_eq!(
            repository
                .projection_input_disposition(&first_input)
                .await
                .unwrap(),
            ProjectionInputDisposition::Duplicate(applied.checkpoint.unwrap())
        );

        let stale = input(
            0,
            b"preflight-stale",
            "preflight-message-0",
            "preflight-cause-0",
            ProjectionGeneration::initial(),
        );
        assert!(matches!(
            repository
                .projection_input_disposition(&stale)
                .await
                .unwrap(),
            ProjectionInputDisposition::Stale(checkpoint)
                if checkpoint.input().position() == 1
        ));
        let corrupted = input(
            1,
            b"preflight-corrupt",
            "preflight-message-1",
            "preflight-cause-1",
            ProjectionGeneration::initial(),
        );
        assert!(matches!(
            repository.projection_input_disposition(&corrupted).await,
            Err(ProjectionProtocolError::InputCorruption)
        ));
        let reused_message = input(
            2,
            b"preflight-two",
            "preflight-message-1",
            "preflight-cause-2",
            ProjectionGeneration::initial(),
        );
        assert!(matches!(
            repository
                .projection_input_disposition(&reused_message)
                .await,
            Err(ProjectionProtocolError::MessageIdReuse { message_id })
                if message_id == "preflight-message-1"
        ));

        let failed_input = input(
            2,
            b"preflight-two",
            "preflight-message-2",
            "preflight-cause-2",
            ProjectionGeneration::initial(),
        );
        repository
            .record_projection_failure(
                ProjectionFailureBatch::new(
                    failed_input.clone(),
                    change_epoch(),
                    "preflight-failure-2",
                    "decode_error",
                    b"bad payload".to_vec(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert!(matches!(
            repository
                .projection_input_disposition(&failed_input)
                .await,
            Err(ProjectionProtocolError::PartitionStopped { failure_id })
                if failure_id == "preflight-failure-2"
        ));

        let generation = repository
            .repair_projection(&topology(), &partition(), "preflight-failure-2")
            .await
            .unwrap();
        let retry = input(
            2,
            b"preflight-two",
            "preflight-message-2",
            "preflight-cause-2",
            generation,
        );
        assert_eq!(
            repository
                .projection_input_disposition(&retry)
                .await
                .unwrap(),
            ProjectionInputDisposition::Pending
        );
        assert!(matches!(
            repository.projection_input_disposition(&first_input).await,
            Err(ProjectionProtocolError::GenerationFenced {
                expected: 2,
                actual: 1
            })
        ));
        assert!(matches!(
            repository
                .projection_input_disposition(&input(
                    3,
                    b"preflight-later",
                    "preflight-message-3",
                    "preflight-cause-3",
                    generation,
                ))
                .await,
            Err(ProjectionProtocolError::IncomparableInput)
        ));

        let repaired = repository
            .commit_projection(batch(retry.clone(), Vec::new(), Vec::new()))
            .await
            .unwrap();
        assert_eq!(
            repository
                .projection_input_disposition(&retry)
                .await
                .unwrap(),
            ProjectionInputDisposition::Duplicate(repaired.checkpoint.unwrap())
        );
    }

    #[tokio::test]
    async fn repair_generation_retries_only_the_exact_failed_input() {
        let repository = repository().await;
        repository
            .commit_projection(batch(
                input(
                    1,
                    b"one",
                    "message-1",
                    "cause-1",
                    ProjectionGeneration::initial(),
                ),
                Vec::new(),
                Vec::new(),
            ))
            .await
            .unwrap();
        let failed_input = input(
            2,
            b"two",
            "message-2",
            "cause-2",
            ProjectionGeneration::initial(),
        );
        repository
            .record_projection_failure(
                ProjectionFailureBatch::new(
                    failed_input.clone(),
                    change_epoch(),
                    "failure-2",
                    "decode_error",
                    b"bad payload".to_vec(),
                )
                .unwrap(),
            )
            .await
            .unwrap();

        let corrupt_stopped = input(
            2,
            b"changed",
            "message-2",
            "cause-2",
            ProjectionGeneration::initial(),
        );
        assert!(matches!(
            repository
                .commit_projection(batch(corrupt_stopped, Vec::new(), Vec::new()))
                .await,
            Err(ProjectionProtocolError::InputCorruption)
        ));

        let generation = repository
            .repair_projection(&topology(), &partition(), "failure-2")
            .await
            .unwrap();
        {
            let protocol = repository.projection_protocol.read().unwrap();
            assert_eq!(
                protocol.generations.get(&GenerationKey {
                    partition: PartitionKey::new(&topology(), &partition()),
                    generation,
                }),
                Some(&GenerationLineage {
                    retry_of_generation: Some(ProjectionGeneration::initial()),
                    retry_of_failure_id: Some("failure-2".into()),
                })
            );
            assert_eq!(
                protocol
                    .partitions
                    .get(&PartitionKey::new(&topology(), &partition()))
                    .and_then(|partition| partition.pending_retry_failure_id.as_deref()),
                Some("failure-2")
            );
        }

        assert!(matches!(
            repository
                .commit_projection(batch(
                    input(
                        1,
                        b"changed-known-cursor",
                        "message-1",
                        "cause-1",
                        ProjectionGeneration::initial(),
                    ),
                    Vec::new(),
                    Vec::new(),
                ))
                .await,
            Err(ProjectionProtocolError::InputCorruption)
        ));
        let changed_source_capability = TrustedProjectionInput::mint(
            input_cursor(3, "source-v1"),
            ProjectionInputFingerprint::from_canonical_bytes(b"changed-source-capability"),
            "changed-source-capability-message",
            "changed-source-capability-cause",
            ProjectionGeneration::initial(),
            false,
        )
        .unwrap();
        assert!(matches!(
            repository
                .commit_projection(batch(changed_source_capability, Vec::new(), Vec::new(),))
                .await,
            Err(ProjectionProtocolError::InputCorruption)
        ));
        assert!(matches!(
            repository
                .commit_projection(batch(
                    other_source_input("unknown-old-generation", ProjectionGeneration::initial()),
                    Vec::new(),
                    Vec::new(),
                ))
                .await,
            Err(ProjectionProtocolError::GenerationFenced {
                expected: 2,
                actual: 1,
            })
        ));
        assert!(matches!(
            repository
                .commit_projection(batch(
                    other_source_input("message-1", ProjectionGeneration::initial()),
                    Vec::new(),
                    Vec::new(),
                ))
                .await,
            Err(ProjectionProtocolError::MessageIdReuse { message_id })
                if message_id == "message-1"
        ));

        let inherited = repository
            .commit_projection(batch(
                input(1, b"one", "message-1", "cause-1", generation),
                Vec::new(),
                Vec::new(),
            ))
            .await
            .unwrap();
        assert_eq!(inherited.outcome, ProjectionCommitOutcome::Duplicate);
        assert!(!repository
            .projection_protocol
            .read()
            .unwrap()
            .applied_receipts
            .contains_key(&CursorReceiptKey::new(
                &input_cursor(1, "source-v1"),
                generation,
            )));
        let inherited_older = repository
            .commit_projection(batch(
                input(0, b"zero", "message-0", "cause-0", generation),
                Vec::new(),
                Vec::new(),
            ))
            .await
            .unwrap();
        assert_eq!(inherited_older.outcome, ProjectionCommitOutcome::StaleInput);

        assert!(matches!(
            repository
                .commit_projection(batch(
                    other_source_input("other-message", generation),
                    Vec::new(),
                    Vec::new(),
                ))
                .await,
            Err(ProjectionProtocolError::IncomparableInput)
        ));
        assert!(matches!(
            repository
                .commit_projection(batch(
                    other_source_input("message-1", generation),
                    Vec::new(),
                    Vec::new(),
                ))
                .await,
            Err(ProjectionProtocolError::MessageIdReuse { message_id })
                if message_id == "message-1"
        ));
        assert!(matches!(
            repository
                .commit_projection(batch(
                    input(2, b"changed", "message-2", "cause-2", generation),
                    Vec::new(),
                    Vec::new(),
                ))
                .await,
            Err(ProjectionProtocolError::InputCorruption)
        ));

        let repaired = repository
            .commit_projection(batch(
                input(2, b"two", "message-2", "cause-2", generation),
                Vec::new(),
                Vec::new(),
            ))
            .await
            .unwrap();
        assert_eq!(repaired.outcome, ProjectionCommitOutcome::Applied);
        assert!(repository
            .projection_protocol
            .read()
            .unwrap()
            .partitions
            .get(&PartitionKey::new(&topology(), &partition()))
            .unwrap()
            .pending_retry_failure_id
            .is_none());
        assert_eq!(
            repository
                .commit_projection(batch(
                    input(3, b"three", "message-3", "cause-3", generation),
                    Vec::new(),
                    Vec::new(),
                ))
                .await
                .unwrap()
                .outcome,
            ProjectionCommitOutcome::Applied
        );

        let failed_again = self::repository().await;
        failed_again
            .record_projection_failure(
                ProjectionFailureBatch::new(
                    input(
                        0,
                        b"first-failure",
                        "first-failure-message",
                        "first-failure-cause",
                        ProjectionGeneration::initial(),
                    ),
                    change_epoch(),
                    "first-failure",
                    "decode_error",
                    b"bad payload".to_vec(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let retry_generation = failed_again
            .repair_projection(&topology(), &partition(), "first-failure")
            .await
            .unwrap();
        let retried_failure = failed_again
            .record_projection_failure(
                ProjectionFailureBatch::new(
                    input(
                        0,
                        b"first-failure",
                        "first-failure-message",
                        "first-failure-cause",
                        retry_generation,
                    ),
                    change_epoch(),
                    "second-failure",
                    "decode_error",
                    b"still bad".to_vec(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(retried_failure.generation, retry_generation);
        assert!(matches!(
            failed_again
                .commit_projection(batch(
                    other_source_input("blocked-after-retry-failure", retry_generation),
                    Vec::new(),
                    Vec::new(),
                ))
                .await,
            Err(ProjectionProtocolError::PartitionStopped { failure_id })
                if failure_id == "second-failure"
        ));
    }

    #[tokio::test]
    async fn tombstone_requires_explicit_exact_recreation() {
        let repository = repository().await;
        let created = repository
            .commit_projection(batch(
                input(
                    1,
                    b"create",
                    "message-1",
                    "cause-1",
                    ProjectionGeneration::initial(),
                ),
                vec![mutation(
                    ProjectionRecordExpectation::Missing,
                    ProjectionMutationKind::Upsert,
                )],
                Vec::new(),
            ))
            .await
            .unwrap()
            .records
            .pop()
            .unwrap();
        let deleted = repository
            .commit_projection(batch(
                input(
                    2,
                    b"delete",
                    "message-2",
                    "cause-2",
                    ProjectionGeneration::initial(),
                ),
                vec![mutation(
                    ProjectionRecordExpectation::Exact(created.revision),
                    ProjectionMutationKind::Delete,
                )],
                Vec::new(),
            ))
            .await
            .unwrap()
            .records
            .pop()
            .unwrap();
        assert!(deleted.tombstone);
        assert!(!row_exists(&repository));

        assert!(matches!(
            repository
                .commit_projection(batch(
                    input(
                        3,
                        b"plain-upsert",
                        "message-3",
                        "cause-3",
                        ProjectionGeneration::initial(),
                    ),
                    vec![mutation(
                        ProjectionRecordExpectation::Exact(deleted.revision.clone()),
                        ProjectionMutationKind::Upsert,
                    )],
                    Vec::new(),
                ))
                .await,
            Err(ProjectionProtocolError::RecordTombstoned { .. })
        ));

        let recreated = repository
            .commit_projection(batch(
                input(
                    3,
                    b"recreate",
                    "message-3b",
                    "cause-3",
                    ProjectionGeneration::initial(),
                ),
                vec![mutation(
                    ProjectionRecordExpectation::Exact(deleted.revision),
                    ProjectionMutationKind::Recreate,
                )],
                Vec::new(),
            ))
            .await
            .unwrap()
            .records
            .pop()
            .unwrap();
        assert_eq!(recreated.revision.incarnation(), 2);
        assert_eq!(recreated.revision.revision(), 1);
        assert!(!recreated.tombstone);
        assert!(row_exists(&repository));
    }

    #[tokio::test]
    async fn physical_rows_must_match_projection_record_metadata() {
        let create_with_orphan_row = repository().await;
        insert_physical_row(&create_with_orphan_row);
        assert!(matches!(
            create_with_orphan_row
                .commit_projection(batch(
                    input(
                        1,
                        b"orphan",
                        "message-orphan",
                        "cause-orphan",
                        ProjectionGeneration::initial(),
                    ),
                    vec![mutation(
                        ProjectionRecordExpectation::Missing,
                        ProjectionMutationKind::Upsert,
                    )],
                    Vec::new(),
                ))
                .await,
            Err(ProjectionProtocolError::InvalidBatch(message))
                if message.contains("physical row to be absent")
        ));
        assert!(create_with_orphan_row
            .projection_record(&record_scope())
            .await
            .unwrap()
            .is_none());
        assert!(create_with_orphan_row
            .inbox_store
            .read()
            .unwrap()
            .is_empty());
        {
            let protocol = create_with_orphan_row.projection_protocol.read().unwrap();
            assert!(protocol.partitions.is_empty());
            assert!(protocol.input_identities.is_empty());
            assert!(protocol.applied_receipts.is_empty());
        }

        for operation in ["save", "patch", "delete"] {
            let repository = repository().await;
            let created = repository
                .commit_projection(batch(
                    input(
                        1,
                        format!("{operation}-create").as_bytes(),
                        &format!("{operation}-message-1"),
                        &format!("{operation}-cause-1"),
                        ProjectionGeneration::initial(),
                    ),
                    vec![mutation(
                        ProjectionRecordExpectation::Missing,
                        ProjectionMutationKind::Upsert,
                    )],
                    Vec::new(),
                ))
                .await
                .unwrap();
            let metadata = created.records[0].clone();
            let changes_before = repository
                .projection_changes(&topology(), &partition(), None, 100)
                .await
                .unwrap();
            let inbox_before = repository.inbox_store.read().unwrap().clone();
            remove_physical_row(&repository);
            let attempted = match operation {
                "save" => mutation(
                    ProjectionRecordExpectation::Exact(metadata.revision.clone()),
                    ProjectionMutationKind::Upsert,
                ),
                "patch" => patch_mutation(metadata.revision.clone()),
                "delete" => mutation(
                    ProjectionRecordExpectation::Exact(metadata.revision.clone()),
                    ProjectionMutationKind::Delete,
                ),
                _ => unreachable!(),
            };
            assert!(matches!(
                repository
                    .commit_projection(batch(
                        input(
                            2,
                            format!("{operation}-missing").as_bytes(),
                            &format!("{operation}-message-2"),
                            &format!("{operation}-cause-2"),
                            ProjectionGeneration::initial(),
                        ),
                        vec![attempted],
                        Vec::new(),
                    ))
                    .await,
                Err(ProjectionProtocolError::InvalidBatch(message))
                    if message.contains("physical row to be present")
            ));
            assert_eq!(
                repository.projection_record(&record_scope()).await.unwrap(),
                Some(metadata)
            );
            assert_eq!(
                repository
                    .projection_changes(&topology(), &partition(), None, 100)
                    .await
                    .unwrap(),
                changes_before
            );
            assert_eq!(*repository.inbox_store.read().unwrap(), inbox_before);
            let protocol = repository.projection_protocol.read().unwrap();
            assert!(!protocol
                .input_identities
                .contains_key(&CursorIdentityKey::new(&input_cursor(2, "source-v1"))));
            assert!(!protocol
                .applied_receipts
                .contains_key(&CursorReceiptKey::new(
                    &input_cursor(2, "source-v1"),
                    ProjectionGeneration::initial(),
                )));
        }

        let recreate_with_row = repository().await;
        let created = recreate_with_row
            .commit_projection(batch(
                input(
                    1,
                    b"create",
                    "recreate-message-1",
                    "recreate-cause-1",
                    ProjectionGeneration::initial(),
                ),
                vec![mutation(
                    ProjectionRecordExpectation::Missing,
                    ProjectionMutationKind::Upsert,
                )],
                Vec::new(),
            ))
            .await
            .unwrap()
            .records
            .remove(0);
        let deleted = recreate_with_row
            .commit_projection(batch(
                input(
                    2,
                    b"delete",
                    "recreate-message-2",
                    "recreate-cause-2",
                    ProjectionGeneration::initial(),
                ),
                vec![mutation(
                    ProjectionRecordExpectation::Exact(created.revision),
                    ProjectionMutationKind::Delete,
                )],
                Vec::new(),
            ))
            .await
            .unwrap()
            .records
            .remove(0);
        insert_physical_row(&recreate_with_row);
        let changes_before = recreate_with_row
            .projection_changes(&topology(), &partition(), None, 100)
            .await
            .unwrap();
        let inbox_before = recreate_with_row.inbox_store.read().unwrap().clone();
        assert!(matches!(
            recreate_with_row
                .commit_projection(batch(
                    input(
                        3,
                        b"recreate",
                        "recreate-message-3",
                        "recreate-cause-3",
                        ProjectionGeneration::initial(),
                    ),
                    vec![mutation(
                        ProjectionRecordExpectation::Exact(deleted.revision.clone()),
                        ProjectionMutationKind::Recreate,
                    )],
                    Vec::new(),
                ))
                .await,
            Err(ProjectionProtocolError::InvalidBatch(message))
                if message.contains("physical row to be absent")
        ));
        assert_eq!(
            recreate_with_row
                .projection_record(&record_scope())
                .await
                .unwrap(),
            Some(deleted)
        );
        assert_eq!(
            recreate_with_row
                .projection_changes(&topology(), &partition(), None, 100)
                .await
                .unwrap(),
            changes_before
        );
        assert_eq!(*recreate_with_row.inbox_store.read().unwrap(), inbox_before);
        assert!(row_exists(&recreate_with_row));
    }

    #[tokio::test]
    async fn table_failure_rolls_back_rows_protocol_and_inbox() {
        let repository = repository().await;
        let invalid = ProjectionRecordMutation::new(
            record_scope(),
            upsert_table_mutation(false),
            ProjectionRecordExpectation::Missing,
            ProjectionMutationKind::Upsert,
        )
        .unwrap();
        assert!(matches!(
            repository
                .commit_projection(batch(
                    input(
                        1,
                        b"invalid",
                        "message-invalid",
                        "cause-invalid",
                        ProjectionGeneration::initial(),
                    ),
                    vec![invalid],
                    Vec::new(),
                ))
                .await,
            Err(ProjectionProtocolError::Table(_))
        ));
        assert!(!row_exists(&repository));
        assert!(repository
            .projection_record(&record_scope())
            .await
            .unwrap()
            .is_none());
        assert!(repository
            .projection_checkpoint(
                &input_cursor(1, "source-v1"),
                ProjectionGeneration::initial()
            )
            .await
            .unwrap()
            .is_none());
        assert!(repository.inbox_store.read().unwrap().is_empty());
        {
            let protocol = repository.projection_protocol.read().unwrap();
            assert!(protocol.partitions.is_empty());
            assert!(protocol.inputs.is_empty());
            assert!(protocol.input_identities.is_empty());
            assert!(protocol.messages.is_empty());
            assert!(protocol.applied_receipts.is_empty());
            assert!(protocol.observations.is_empty());
            assert!(protocol.failures.is_empty());
        }

        let changes = repository
            .projection_changes(&topology(), &partition(), None, 100)
            .await
            .unwrap();
        assert_eq!(
            changes,
            ProjectionChangeRead::Changes {
                head: None,
                compacted_through: 0,
                changes: Vec::new(),
            }
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_same_input_applies_once() {
        let repository = repository().await;
        let barrier = Arc::new(Barrier::new(2));
        let mut joins = Vec::new();
        for _ in 0..2 {
            let repository = repository.clone();
            let barrier = Arc::clone(&barrier);
            joins.push(tokio::spawn(async move {
                barrier.wait();
                repository
                    .commit_projection(batch(
                        input(
                            1,
                            b"same-input",
                            "message-1",
                            "cause-1",
                            ProjectionGeneration::initial(),
                        ),
                        vec![mutation(
                            ProjectionRecordExpectation::Missing,
                            ProjectionMutationKind::Upsert,
                        )],
                        Vec::new(),
                    ))
                    .await
                    .unwrap()
                    .outcome
            }));
        }
        let mut outcomes = Vec::with_capacity(joins.len());
        for join in joins {
            outcomes.push(join.await.unwrap());
        }
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| **outcome == ProjectionCommitOutcome::Applied)
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| **outcome == ProjectionCommitOutcome::Duplicate)
                .count(),
            1
        );
        assert_eq!(
            repository
                .projection_record(&record_scope())
                .await
                .unwrap()
                .unwrap()
                .revision
                .revision(),
            1
        );
    }

    #[tokio::test]
    async fn observations_are_immutable_and_staged_records_reuse_row_changes() {
        let repository = repository().await;
        let scope = record_scope();
        let first = repository
            .commit_projection(batch(
                input(
                    1,
                    b"first",
                    "message-1",
                    "cause-stable",
                    ProjectionGeneration::initial(),
                ),
                vec![mutation(
                    ProjectionRecordExpectation::Missing,
                    ProjectionMutationKind::Upsert,
                )],
                vec![ProjectionObservationRequest {
                    kind: ProjectionObservationKind::Record,
                    target: ProjectionObservationTarget::StagedRecord(scope.clone()),
                }],
            ))
            .await
            .unwrap();
        assert_eq!(first.changes.len(), 1);
        assert_eq!(first.changes[0].kind, ProjectionChangeKind::RecordUpsert);
        let earliest = repository
            .projection_observation("cause-stable", &scope, ProjectionObservationKind::Record)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(earliest.change, first.changes[0].cursor);
        assert_eq!(earliest.revision.as_ref(), Some(&first.records[0].revision));

        let second = repository
            .commit_projection(batch(
                input(
                    2,
                    b"second",
                    "message-2",
                    "cause-stable",
                    ProjectionGeneration::initial(),
                ),
                vec![mutation(
                    ProjectionRecordExpectation::Exact(first.records[0].revision.clone()),
                    ProjectionMutationKind::Upsert,
                )],
                vec![ProjectionObservationRequest {
                    kind: ProjectionObservationKind::Record,
                    target: ProjectionObservationTarget::StagedRecord(scope.clone()),
                }],
            ))
            .await
            .unwrap();
        assert_eq!(second.changes.len(), 1);
        assert_eq!(second.changes[0].kind, ProjectionChangeKind::RecordUpsert);
        assert_eq!(
            repository
                .projection_observation("cause-stable", &scope, ProjectionObservationKind::Record)
                .await
                .unwrap(),
            Some(earliest)
        );

        let existing = repository
            .commit_projection(batch(
                input(
                    3,
                    b"existing",
                    "message-3",
                    "cause-existing",
                    ProjectionGeneration::initial(),
                ),
                Vec::new(),
                vec![ProjectionObservationRequest {
                    kind: ProjectionObservationKind::Record,
                    target: ProjectionObservationTarget::ExistingRecord(
                        second.records[0].revision.clone(),
                    ),
                }],
            ))
            .await
            .unwrap();
        assert_eq!(existing.changes.len(), 1);
        assert_eq!(existing.changes[0].kind, ProjectionChangeKind::Observation);
        let earliest_existing = repository
            .projection_observation("cause-existing", &scope, ProjectionObservationKind::Record)
            .await
            .unwrap()
            .unwrap();

        let repeated = repository
            .commit_projection(batch(
                input(
                    4,
                    b"existing-repeat",
                    "message-4",
                    "cause-existing",
                    ProjectionGeneration::initial(),
                ),
                Vec::new(),
                vec![ProjectionObservationRequest {
                    kind: ProjectionObservationKind::Record,
                    target: ProjectionObservationTarget::ExistingRecord(
                        second.records[0].revision.clone(),
                    ),
                }],
            ))
            .await
            .unwrap();
        assert_eq!(repeated.changes.len(), 1);
        assert_eq!(repeated.changes[0].kind, ProjectionChangeKind::Checkpoint);
        assert_eq!(
            repository
                .projection_observation("cause-existing", &scope, ProjectionObservationKind::Record)
                .await
                .unwrap(),
            Some(earliest_existing)
        );
    }

    #[tokio::test]
    async fn dependency_failure_repair_and_resume_are_durable() {
        let repository = repository().await;
        let scope = record_scope();
        let first = repository
            .commit_projection(batch(
                input(
                    1,
                    b"dependency",
                    "message-1",
                    "cause-1",
                    ProjectionGeneration::initial(),
                ),
                Vec::new(),
                vec![ProjectionObservationRequest {
                    kind: ProjectionObservationKind::Dependency,
                    target: ProjectionObservationTarget::Dependency(scope.clone()),
                }],
            ))
            .await
            .unwrap();
        let observation = repository
            .projection_observation("cause-1", &scope, ProjectionObservationKind::Dependency)
            .await
            .unwrap()
            .unwrap();
        assert!(observation.revision.is_none());
        assert_eq!(observation.scope, scope);

        let failure_batch = ProjectionFailureBatch::new(
            input(
                2,
                b"failure",
                "message-2",
                "cause-2",
                ProjectionGeneration::initial(),
            ),
            change_epoch(),
            "failure-2",
            "decode_error",
            b"bad payload".to_vec(),
        )
        .unwrap();
        let failure = repository
            .record_projection_failure(failure_batch.clone())
            .await
            .unwrap();
        assert_eq!(
            repository
                .record_projection_failure(failure_batch.clone())
                .await
                .unwrap(),
            failure
        );
        let changed_capability = ProjectionFailureBatch::new(
            TrustedProjectionInput::mint(
                input_cursor(2, "source-v1"),
                ProjectionInputFingerprint::from_canonical_bytes(b"failure"),
                "message-2",
                "cause-2",
                ProjectionGeneration::initial(),
                false,
            )
            .unwrap(),
            change_epoch(),
            "failure-2",
            "decode_error",
            b"bad payload".to_vec(),
        )
        .unwrap();
        assert!(matches!(
            repository
                .record_projection_failure(changed_capability)
                .await,
            Err(ProjectionProtocolError::InputCorruption)
        ));
        assert_eq!(
            repository
                .projection_failure(&topology(), &partition(), "failure-2")
                .await
                .unwrap(),
            Some(failure.clone())
        );
        assert!(matches!(
            repository
                .commit_projection(batch(
                    input(
                        3,
                        b"blocked",
                        "message-3",
                        "cause-3",
                        ProjectionGeneration::initial(),
                    ),
                    Vec::new(),
                    Vec::new(),
                ))
                .await,
            Err(ProjectionProtocolError::PartitionStopped { .. })
        ));

        let generation = repository
            .repair_projection(&topology(), &partition(), "failure-2")
            .await
            .unwrap();
        assert_eq!(generation.get(), 2);
        assert!(matches!(
            repository.record_projection_failure(failure_batch).await,
            Err(ProjectionProtocolError::GenerationFenced {
                expected: 2,
                actual: 1
            })
        ));
        let repaired = repository
            .commit_projection(batch(
                input(2, b"failure", "message-2", "cause-2", generation),
                Vec::new(),
                Vec::new(),
            ))
            .await
            .unwrap();
        assert_eq!(repaired.changes[0].kind, ProjectionChangeKind::Checkpoint);

        let compacted = repository
            .compact_projection_changes(&first.changes[0].cursor)
            .await
            .unwrap();
        assert_eq!(compacted, first.changes[0].cursor.position());
        assert!(matches!(
            repository
                .projection_changes(&topology(), &partition(), None, 100)
                .await
                .unwrap(),
            ProjectionChangeRead::ResetRequired {
                compacted_through,
                ..
            } if compacted_through == compacted
        ));
        let boundary_resume = repository
            .projection_changes(
                &topology(),
                &partition(),
                Some(&first.changes[0].cursor),
                100,
            )
            .await
            .unwrap();
        assert!(matches!(
            boundary_resume,
            ProjectionChangeRead::Changes {
                compacted_through,
                ref changes,
                ..
            } if compacted_through == compacted && changes.len() == 2
        ));

        repository
            .compact_projection_changes(&failure.change)
            .await
            .unwrap();
        assert!(matches!(
            repository
                .projection_changes(
                    &topology(),
                    &partition(),
                    Some(&first.changes[0].cursor),
                    100
                )
                .await
                .unwrap(),
            ProjectionChangeRead::ResetRequired { .. }
        ));

        let future = ProjectionChangeCursor::new(
            topology(),
            partition(),
            change_epoch(),
            repaired.changes[0].cursor.position() + 10,
        )
        .unwrap();
        assert!(matches!(
            repository
                .projection_changes(&topology(), &partition(), Some(&future), 100)
                .await
                .unwrap(),
            ProjectionChangeRead::ResetRequired { .. }
        ));
    }

    #[tokio::test]
    async fn causal_owned_table_rejects_every_legacy_commit_path() {
        let repository = repository().await;
        let before_first_message = repository
            .model_store()
            .commit_write_plan(TableWritePlan::new(vec![upsert_table_mutation(true)]))
            .await
            .unwrap_err();
        assert!(matches!(
            before_first_message,
            TableStoreError::CausalWriteRequired { ref table } if table == "todo_views"
        ));
        repository
            .commit_projection(batch(
                input(
                    1,
                    b"claim",
                    "message-1",
                    "cause-1",
                    ProjectionGeneration::initial(),
                ),
                vec![mutation(
                    ProjectionRecordExpectation::Missing,
                    ProjectionMutationKind::Upsert,
                )],
                Vec::new(),
            ))
            .await
            .unwrap();

        let direct = repository
            .commit_write_plan(TableWritePlan::new(vec![upsert_table_mutation(true)]))
            .await
            .unwrap_err();
        assert!(matches!(
            direct,
            TableStoreError::CausalWriteRequired { ref table } if table == "todo_views"
        ));
        let bare_handle = repository
            .model_store()
            .commit_write_plan(TableWritePlan::new(vec![upsert_table_mutation(true)]))
            .await
            .unwrap_err();
        assert!(matches!(
            bare_handle,
            TableStoreError::CausalWriteRequired { ref table } if table == "todo_views"
        ));

        let mut raw_batch = CommitBatch::empty();
        raw_batch
            .read_model_plans
            .push(TableWritePlan::new(vec![upsert_table_mutation(true)]));
        let transactional = repository.commit_batch(raw_batch).await.unwrap_err();
        assert!(matches!(
            transactional,
            RepositoryError::CausalWriteRequired { ref table } if table == "todo_views"
        ));
    }
}
