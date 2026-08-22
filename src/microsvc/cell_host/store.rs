//! Per-shard stream store: in-process stand-in for one cell's private SQLite.
//!
//! Production celld wraps rusqlite (or workers-rs storage) the same way: sync
//! calls inside async fns. This is **not** `feature = "sqlite"` (sqlx pool) and
//! **not** a `celld` dialect.

use std::future::Future;

use crate::command_ledger::{
    AttemptFence, CausalCommitBatch, CausalGetStream, CausalRepositoryIdentity,
    CausalStorageIdentity, CausalTransactionalCommit, CommandLedgerError, CommandLedgerKey,
    CommandLedgerStore, CommandLookup, CommandLookupScope, CommandReservation, ReservationOutcome,
};
use crate::entity::Entity;
use crate::microsvc::HasOutboxStore;
use crate::projection_protocol::{
    ProjectionChangeCursor, ProjectionChangeRead, ProjectionCheckpoint, ProjectionCommitBatch,
    ProjectionCommitResult, ProjectionFailure, ProjectionFailureBatch, ProjectionFailureLocation,
    ProjectionGeneration, ProjectionInputCursor, ProjectionInputDisposition,
    ProjectionLiveRecordBatch, ProjectionLiveRecordBatchRequest, ProjectionModelOwnership,
    ProjectionObligationEvidenceBatch, ProjectionObligationEvidenceBatchRequest,
    ProjectionObservation, ProjectionObservationKind, ProjectionPartition,
    ProjectionPartitionRuntimeState, ProjectionProtocolError, ProjectionProtocolStore,
    ProjectionQuerySnapshot, ProjectionQuerySnapshotBatch, ProjectionQuerySnapshotBatchRequest,
    ProjectionQuerySnapshotRequest, ProjectionRecordMetadata, ProjectionRecordScope,
    ProjectorTopologyId, TrustedProjectionInput,
};
use crate::repository::{
    CommitBatch, RepositoryError, SnapshotWrite, StreamIdentity, TransactionalCommit,
};
use crate::{InMemoryOutboxStore, InMemoryRepository};

/// Private SQLite stand-in for one cell instance (`{aggregate_type}:{shard}`).
///
/// Loads and commits are rejected for any stream that is not this cell's shard.
#[derive(Clone)]
pub struct CellStreamStore {
    identity: StreamIdentity,
    inner: InMemoryRepository,
}

impl CellStreamStore {
    /// Bind a store to one exact stream identity.
    pub fn for_identity(identity: StreamIdentity) -> Self {
        Self {
            identity,
            inner: InMemoryRepository::new(),
        }
    }

    /// Named cell constructor used by [`super::AggregateCell`].
    pub fn new(
        aggregate_type: impl Into<String>,
        shard_id: impl Into<String>,
    ) -> Result<Self, RepositoryError> {
        Ok(Self::for_identity(StreamIdentity::new(
            aggregate_type,
            shard_id,
        )?))
    }

    /// Cell instance name (`type:id`).
    pub fn instance_name(&self) -> String {
        self.identity.to_string()
    }

    /// Stream this cell owns.
    pub fn identity(&self) -> &StreamIdentity {
        &self.identity
    }

    fn ensure_identity(&self, identity: &StreamIdentity) -> Result<(), RepositoryError> {
        if identity != &self.identity {
            return Err(RepositoryError::Model(format!(
                "cell `{}` cannot access stream `{identity}`",
                self.identity
            )));
        }
        Ok(())
    }

    fn ensure_batch(&self, batch: &CommitBatch<'_>) -> Result<(), RepositoryError> {
        for stream in &batch.streams {
            self.ensure_identity(&stream.identity)?;
        }
        for snapshot in &batch.snapshots {
            match snapshot {
                SnapshotWrite::Save { identity, .. } => self.ensure_identity(identity)?,
            }
        }
        Ok(())
    }
}

impl CausalGetStream for CellStreamStore {
    fn get_causal_stream<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<Option<Entity>, RepositoryError>> + Send + 'a {
        async move {
            self.ensure_identity(identity)?;
            CausalGetStream::get_causal_stream(&self.inner, identity).await
        }
    }
}

impl CausalRepositoryIdentity for CellStreamStore {
    fn causal_storage_identity(&self) -> CausalStorageIdentity {
        CausalRepositoryIdentity::causal_storage_identity(&self.inner)
    }
}

impl CommandLedgerStore for CellStreamStore {
    fn reserve_command(
        &self,
        reservation: CommandReservation,
    ) -> impl Future<Output = Result<ReservationOutcome, CommandLedgerError>> + Send + '_ {
        CommandLedgerStore::reserve_command(&self.inner, reservation)
    }

    fn lookup_command<'a>(
        &'a self,
        key: &'a CommandLedgerKey,
        scope: CommandLookupScope<'a>,
    ) -> impl Future<Output = Result<CommandLookup, CommandLedgerError>> + Send + 'a {
        CommandLedgerStore::lookup_command(&self.inner, key, scope)
    }

    fn mark_retryable_unknown(
        &self,
        attempt: AttemptFence,
    ) -> impl Future<Output = Result<(), CommandLedgerError>> + Send + '_ {
        CommandLedgerStore::mark_retryable_unknown(&self.inner, attempt)
    }

    fn compact_expired_commands(
        &self,
        limit: usize,
    ) -> impl Future<Output = Result<u64, CommandLedgerError>> + Send + '_ {
        CommandLedgerStore::compact_expired_commands(&self.inner, limit)
    }
}

impl TransactionalCommit for CellStreamStore {
    fn commit_batch<'a>(
        &'a self,
        batch: CommitBatch<'a>,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        async move {
            self.ensure_batch(&batch)?;
            TransactionalCommit::commit_batch(&self.inner, batch).await
        }
    }
}

impl CausalTransactionalCommit for CellStreamStore {
    fn commit_causal_batch<'a>(
        &'a self,
        batch: CausalCommitBatch<'a>,
    ) -> impl Future<Output = Result<(), CommandLedgerError>> + Send + 'a {
        async move {
            self.ensure_batch(&batch.domain)
                .map_err(CommandLedgerError::Storage)?;
            CausalTransactionalCommit::commit_causal_batch(&self.inner, batch).await
        }
    }
}

impl HasOutboxStore for CellStreamStore {
    type OutboxStore = InMemoryOutboxStore;

    fn outbox_store(&self) -> Self::OutboxStore {
        self.inner.outbox_store()
    }
}

impl ProjectionProtocolStore for CellStreamStore {
    fn register_projection_models<'a>(
        &'a self,
        topology: &'a ProjectorTopologyId,
        ownership: &'a [ProjectionModelOwnership],
    ) -> impl Future<Output = Result<(), ProjectionProtocolError>> + Send + 'a {
        self.inner.register_projection_models(topology, ownership)
    }

    fn commit_projection(
        &self,
        batch: ProjectionCommitBatch,
    ) -> impl Future<Output = Result<ProjectionCommitResult, ProjectionProtocolError>> + Send + '_
    {
        self.inner.commit_projection(batch)
    }

    fn record_projection_failure(
        &self,
        batch: ProjectionFailureBatch,
    ) -> impl Future<Output = Result<ProjectionFailure, ProjectionProtocolError>> + Send + '_ {
        self.inner.record_projection_failure(batch)
    }

    fn projection_checkpoint<'a>(
        &'a self,
        cursor_scope: &'a ProjectionInputCursor,
        generation: ProjectionGeneration,
    ) -> impl Future<Output = Result<Option<ProjectionCheckpoint>, ProjectionProtocolError>> + Send + 'a
    {
        self.inner.projection_checkpoint(cursor_scope, generation)
    }

    fn projection_record<'a>(
        &'a self,
        scope: &'a ProjectionRecordScope,
    ) -> impl Future<Output = Result<Option<ProjectionRecordMetadata>, ProjectionProtocolError>>
           + Send
           + 'a {
        self.inner.projection_record(scope)
    }

    fn projection_input_disposition<'a>(
        &'a self,
        input: &'a TrustedProjectionInput,
    ) -> impl Future<Output = Result<ProjectionInputDisposition, ProjectionProtocolError>> + Send + 'a
    {
        self.inner.projection_input_disposition(input)
    }

    fn projection_query_snapshot<'a>(
        &'a self,
        request: &'a ProjectionQuerySnapshotRequest,
    ) -> impl Future<Output = Result<ProjectionQuerySnapshot, ProjectionProtocolError>> + Send + 'a
    {
        self.inner.projection_query_snapshot(request)
    }

    fn projection_query_snapshot_batch<'a>(
        &'a self,
        request: &'a ProjectionQuerySnapshotBatchRequest,
    ) -> impl Future<Output = Result<ProjectionQuerySnapshotBatch, ProjectionProtocolError>> + Send + 'a
    {
        self.inner.projection_query_snapshot_batch(request)
    }

    fn projection_obligation_evidence_batch<'a>(
        &'a self,
        request: &'a ProjectionObligationEvidenceBatchRequest,
    ) -> impl Future<Output = Result<ProjectionObligationEvidenceBatch, ProjectionProtocolError>>
           + Send
           + 'a {
        self.inner.projection_obligation_evidence_batch(request)
    }

    fn projection_live_record_batch<'a>(
        &'a self,
        request: &'a ProjectionLiveRecordBatchRequest,
    ) -> impl Future<Output = Result<ProjectionLiveRecordBatch, ProjectionProtocolError>> + Send + 'a
    {
        self.inner.projection_live_record_batch(request)
    }

    fn projection_partition_runtime_state<'a>(
        &'a self,
        topology: &'a ProjectorTopologyId,
        partition: &'a ProjectionPartition,
    ) -> impl Future<Output = Result<Option<ProjectionPartitionRuntimeState>, ProjectionProtocolError>>
           + Send
           + 'a {
        self.inner
            .projection_partition_runtime_state(topology, partition)
    }

    fn projection_observation<'a>(
        &'a self,
        causation_id: &'a str,
        scope: &'a ProjectionRecordScope,
        kind: ProjectionObservationKind,
    ) -> impl Future<Output = Result<Option<ProjectionObservation>, ProjectionProtocolError>> + Send + 'a
    {
        self.inner.projection_observation(causation_id, scope, kind)
    }

    fn projection_changes<'a>(
        &'a self,
        topology: &'a ProjectorTopologyId,
        partition: &'a ProjectionPartition,
        after: Option<&'a ProjectionChangeCursor>,
        limit: usize,
    ) -> impl Future<Output = Result<ProjectionChangeRead, ProjectionProtocolError>> + Send + 'a
    {
        self.inner
            .projection_changes(topology, partition, after, limit)
    }

    fn repair_projection<'a>(
        &'a self,
        topology: &'a ProjectorTopologyId,
        partition: &'a ProjectionPartition,
        failure_id: &'a str,
    ) -> impl Future<Output = Result<ProjectionGeneration, ProjectionProtocolError>> + Send + 'a
    {
        self.inner
            .repair_projection(topology, partition, failure_id)
    }

    fn compact_projection_changes<'a>(
        &'a self,
        through: &'a ProjectionChangeCursor,
    ) -> impl Future<Output = Result<u64, ProjectionProtocolError>> + Send + 'a {
        self.inner.compact_projection_changes(through)
    }

    fn projection_failure<'a>(
        &'a self,
        topology: &'a ProjectorTopologyId,
        partition: &'a ProjectionPartition,
        failure_id: &'a str,
    ) -> impl Future<Output = Result<Option<ProjectionFailure>, ProjectionProtocolError>> + Send + 'a
    {
        self.inner
            .projection_failure(topology, partition, failure_id)
    }

    fn projection_failure_location<'a>(
        &'a self,
        failure_id: &'a str,
    ) -> impl Future<Output = Result<Option<ProjectionFailureLocation>, ProjectionProtocolError>>
           + Send
           + 'a {
        self.inner.projection_failure_location(failure_id)
    }
}
