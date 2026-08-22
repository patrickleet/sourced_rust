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
use crate::entity::{Entity, EventRecord};
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
    CommitBatch, GetStream, RepositoryError, SnapshotWrite, StreamIdentity, TransactionalCommit,
};
use crate::{InMemoryOutboxStore, InMemoryRepository};
use serde::{Deserialize, Serialize};

#[derive(Clone)]
enum CellOwnership {
    /// One stream identity (Todo, BlobGame). Foreign streams are rejected.
    Exclusive(StreamIdentity),
    /// Parent game cell: map/player/bomb/explosion/saga streams share this
    /// cell's private SQLite. There is no API to commit across two cells.
    Parent { name: StreamIdentity },
}

/// Private SQLite stand-in for one cell instance (`{aggregate_type}:{shard}`).
///
/// Exclusive cells reject any stream that is not this cell's shard. Parent
/// cells (`for_parent_shard`) hold sibling streams of one game and commit
/// them in one [`CommitBatch`].
///
/// ```compile_fail
/// fn two_cell_transaction_does_not_exist(
///     left: &distributed::cell_host::CellStreamStore,
///     right: &distributed::cell_host::CellStreamStore,
///     batch: distributed::CommitBatch<'_>,
/// ) {
///     let _ = left.commit_across(right, batch);
/// }
/// ```

/// One stream's event records for Durable Object SQLite persistence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DurableCellEvents {
    pub stream: String,
    pub events: Vec<EventRecord>,
}

#[derive(Clone)]
pub struct CellStreamStore {
    ownership: CellOwnership,
    inner: InMemoryRepository,
}

impl CellStreamStore {
    /// Bind a store to one exact stream identity.
    pub fn for_identity(identity: StreamIdentity) -> Self {
        Self {
            ownership: CellOwnership::Exclusive(identity),
            inner: InMemoryRepository::new(),
        }
    }

    /// Parent-shard cell: `"{parent_type}:{parent_id}"` (bomberman `game:{id}`).
    ///
    /// Child streams of any aggregate type live in this cell's SQLite. A
    /// transaction across two parent cells does not exist.
    pub fn for_parent_shard(
        parent_type: impl Into<String>,
        parent_id: impl Into<String>,
    ) -> Result<Self, RepositoryError> {
        Ok(Self {
            ownership: CellOwnership::Parent {
                name: StreamIdentity::new(parent_type, parent_id)?,
            },
            inner: InMemoryRepository::new(),
        })
    }

    /// Named exclusive-cell constructor used by [`super::AggregateCell`].
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
        match &self.ownership {
            CellOwnership::Exclusive(identity) | CellOwnership::Parent { name: identity } => {
                identity.to_string()
            }
        }
    }

    /// Stream this exclusive cell owns. Parent cells have no single stream.
    pub fn identity(&self) -> Option<&StreamIdentity> {
        match &self.ownership {
            CellOwnership::Exclusive(identity) => Some(identity),
            CellOwnership::Parent { .. } => None,
        }
    }

    fn ensure_identity(&self, identity: &StreamIdentity) -> Result<(), RepositoryError> {
        match &self.ownership {
            CellOwnership::Parent { .. } => Ok(()),
            CellOwnership::Exclusive(owned) if identity == owned => Ok(()),
            CellOwnership::Exclusive(owned) => Err(RepositoryError::Model(format!(
                "cell `{owned}` cannot access stream `{identity}`"
            ))),
        }
    }

    /// Event log for Durable Object SQLite. Memory remains the working copy.
    pub fn durable_events(&self) -> Result<Vec<DurableCellEvents>, RepositoryError> {
        Ok(self
            .inner
            .clone_events()?
            .into_iter()
            .map(|(stream, events)| DurableCellEvents { stream, events })
            .collect())
    }

    /// Replace the working event log from Durable Object SQLite.
    pub fn restore_durable_events(
        &self,
        events: Vec<DurableCellEvents>,
    ) -> Result<(), RepositoryError> {
        self.inner.replace_events(
            events
                .into_iter()
                .map(|row| (row.stream, row.events))
                .collect(),
        )
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

impl GetStream for CellStreamStore {
    fn get_stream<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<Option<Entity>, RepositoryError>> + Send + 'a {
        async move {
            self.ensure_identity(identity)?;
            GetStream::get_stream(&self.inner, identity).await
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
