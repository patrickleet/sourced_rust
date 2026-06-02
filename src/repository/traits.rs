use std::future::Future;

use crate::entity::{Entity, EventRecord};
use crate::outbox::OutboxMessage;
use crate::read_model::{
    ReadModelAdapterCapabilities, ReadModelCommitOutcome, ReadModelError, ReadModelLoadGraph,
    ReadModelLoadRequest, ReadModelQueryCapabilities, ReadModelWritePlan,
};
use crate::snapshot::SnapshotRecord;

use super::inbox::InboxReceipt;
use super::{RepositoryError, StreamIdentity};

/// One aggregate event stream staged for an async transactional commit.
pub struct StreamWrite<'a> {
    pub identity: StreamIdentity,
    pub entity: &'a mut Entity,
}

impl<'a> StreamWrite<'a> {
    pub fn new(identity: StreamIdentity, entity: &'a mut Entity) -> Self {
        Self { identity, entity }
    }
}

/// Snapshot writes staged in an async transactional commit.
#[derive(Clone, Debug)]
pub enum SnapshotWrite {
    Save {
        identity: StreamIdentity,
        record: SnapshotRecord,
    },
}

/// A structured async write batch that must commit under one backend transaction.
pub struct CommitBatch<'a> {
    pub streams: Vec<StreamWrite<'a>>,
    pub outbox_messages: Vec<OutboxMessage>,
    pub read_model_plans: Vec<ReadModelWritePlan>,
    pub snapshots: Vec<SnapshotWrite>,
    /// Consumer inbox receipts to record in the same transaction (the optional
    /// effectively-once effect fence). Empty for the default idempotent path.
    pub inbox_receipts: Vec<InboxReceipt>,
}

impl<'a> CommitBatch<'a> {
    pub fn new(streams: Vec<StreamWrite<'a>>) -> Self {
        Self {
            streams,
            outbox_messages: Vec::new(),
            read_model_plans: Vec::new(),
            snapshots: Vec::new(),
            inbox_receipts: Vec::new(),
        }
    }

    pub fn empty() -> Self {
        Self::new(Vec::new())
    }
}

/// Owned append data prepared from a borrowed stream write before async I/O.
#[derive(Clone, Debug)]
pub struct PreparedEventAppend {
    pub identity: StreamIdentity,
    pub expected_version: u64,
    pub events: Vec<EventRecord>,
}

impl PreparedEventAppend {
    pub fn from_stream_write(write: &StreamWrite<'_>) -> Self {
        Self {
            identity: write.identity.clone(),
            expected_version: write.entity.committed_version(),
            events: write.entity.new_events().to_vec(),
        }
    }
}

/// Async stream-aware aggregate loading.
pub trait GetStream: Send + Sync {
    fn get_stream<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<Option<Entity>, RepositoryError>> + Send + 'a;

    fn get_streams<'a>(
        &'a self,
        identities: &'a [StreamIdentity],
    ) -> impl Future<Output = Result<Vec<Entity>, RepositoryError>> + Send + 'a;
}

/// Async transactional commit capability for durable persistence backends.
pub trait TransactionalCommit: Send + Sync {
    fn commit_batch<'a>(
        &'a self,
        batch: CommitBatch<'a>,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a;
}

/// Consumer inbox read capability: check whether a `(consumer, message_id)`
/// receipt has already been recorded.
///
/// The pre-check lets a consumer skip re-running a handler for an already-processed
/// message (and ack the redelivery) before opening a transaction. The
/// authoritative dedupe is still the receipt's `(consumer, message_id)` primary
/// key written in [`commit_batch`](TransactionalCommit::commit_batch),
/// which fences the race where two deliveries both pass the pre-check.
pub trait InboxStore: Send + Sync {
    fn inbox_contains<'a>(
        &'a self,
        consumer: &'a str,
        message_id: &'a str,
    ) -> impl Future<Output = Result<bool, RepositoryError>> + Send + 'a;
}

/// Repository trait for types that implement async stream reads and commits.
pub trait Repository: GetStream + TransactionalCommit {}

impl<T> Repository for T where T: GetStream + TransactionalCommit {}

/// Async adapter contract for committing read-model write plans.
pub trait ReadModelWritePlanStore: Send + Sync {
    fn read_model_capabilities(&self) -> ReadModelAdapterCapabilities;

    fn commit_write_plan(
        &self,
        plan: ReadModelWritePlan,
    ) -> impl Future<Output = Result<ReadModelCommitOutcome, ReadModelError>> + Send + '_;
}

/// Async primary-key relational read-model query contract.
pub trait RelationalReadModelQueryStore: Send + Sync {
    fn read_model_query_capabilities(&self) -> ReadModelQueryCapabilities;

    fn load_graph(
        &self,
        request: ReadModelLoadRequest,
    ) -> impl Future<Output = Result<ReadModelLoadGraph, ReadModelError>> + Send + '_;
}

/// Async snapshot persistence keyed by full stream identity.
pub trait SnapshotStore: Send + Sync {
    fn get_snapshot<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<Option<SnapshotRecord>, RepositoryError>> + Send + 'a;

    fn save_snapshot<'a>(
        &'a self,
        identity: &'a StreamIdentity,
        record: SnapshotRecord,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a;

    fn delete_snapshot<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<bool, RepositoryError>> + Send + 'a;
}
