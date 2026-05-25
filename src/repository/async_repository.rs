use std::future::Future;
use std::time::Duration;

use crate::entity::{Entity, EventRecord};
use crate::outbox::{OutboxMessage, OutboxMessageStatus};
use crate::outbox_worker::OutboxPublishFailureAction;
use crate::read_model::{
    ReadModel, ReadModelAdapterCapabilities, ReadModelCommitOutcome, ReadModelError,
    ReadModelLoadGraph, ReadModelLoadRequest, ReadModelQueryCapabilities, ReadModelWritePlan,
    Versioned,
};
use crate::snapshot::SnapshotRecord;

use super::{RepositoryError, StreamIdentity};

/// One aggregate event stream staged for an async transactional commit.
pub struct AsyncStreamWrite<'a> {
    pub identity: StreamIdentity,
    pub entity: &'a mut Entity,
}

impl<'a> AsyncStreamWrite<'a> {
    pub fn new(identity: StreamIdentity, entity: &'a mut Entity) -> Self {
        Self { identity, entity }
    }
}

/// Snapshot writes staged in an async transactional commit.
#[derive(Clone, Debug)]
pub enum AsyncSnapshotWrite {
    Save {
        identity: StreamIdentity,
        record: SnapshotRecord,
    },
}

/// A structured async write batch that must commit under one backend transaction.
pub struct AsyncCommitBatch<'a> {
    pub streams: Vec<AsyncStreamWrite<'a>>,
    pub outbox_messages: Vec<OutboxMessage>,
    pub read_model_plans: Vec<ReadModelWritePlan>,
    pub snapshots: Vec<AsyncSnapshotWrite>,
}

impl<'a> AsyncCommitBatch<'a> {
    pub fn new(streams: Vec<AsyncStreamWrite<'a>>) -> Self {
        Self {
            streams,
            outbox_messages: Vec::new(),
            read_model_plans: Vec::new(),
            snapshots: Vec::new(),
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
    pub fn from_stream_write(write: &AsyncStreamWrite<'_>) -> Self {
        Self {
            identity: write.identity.clone(),
            expected_version: write.entity.committed_version(),
            events: write.entity.new_events().to_vec(),
        }
    }
}

/// Async stream-aware aggregate loading.
pub trait AsyncGetStream: Send + Sync {
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
pub trait AsyncTransactionalCommit: Send + Sync {
    fn commit_batch_async<'a>(
        &'a self,
        batch: AsyncCommitBatch<'a>,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a;
}

/// Repository trait for types that implement async stream reads and commits.
pub trait AsyncRepository: AsyncGetStream + AsyncTransactionalCommit {}

impl<T> AsyncRepository for T where T: AsyncGetStream + AsyncTransactionalCommit {}

/// Async CRUD storage for document-style read models.
pub trait AsyncReadModelStore: Send + Sync {
    fn get_model_async<'a, M>(
        &'a self,
        id: &'a str,
    ) -> impl Future<Output = Result<Option<Versioned<M>>, ReadModelError>> + Send + 'a
    where
        M: ReadModel + 'a;

    fn get_by_primary_key_async<'a, M>(
        &'a self,
        id: &'a str,
    ) -> impl Future<Output = Result<Option<Versioned<M>>, ReadModelError>> + Send + 'a
    where
        M: ReadModel + 'a;

    fn upsert_async<'a, M>(
        &'a self,
        model: &'a M,
    ) -> impl Future<Output = Result<Versioned<M>, ReadModelError>> + Send + 'a
    where
        M: ReadModel + 'a;

    fn insert_async<'a, M>(
        &'a self,
        model: &'a M,
    ) -> impl Future<Output = Result<Versioned<M>, ReadModelError>> + Send + 'a
    where
        M: ReadModel + 'a;

    fn update_async<'a, M>(
        &'a self,
        model: &'a M,
        expected_version: u64,
    ) -> impl Future<Output = Result<Versioned<M>, ReadModelError>> + Send + 'a
    where
        M: ReadModel + 'a;

    fn delete_async<'a, M>(
        &'a self,
        id: &'a str,
    ) -> impl Future<Output = Result<bool, ReadModelError>> + Send + 'a
    where
        M: ReadModel + 'a;
}

/// Async adapter contract for committing read-model write plans.
pub trait AsyncReadModelSessionStore: Send + Sync {
    fn read_model_capabilities_async(&self) -> ReadModelAdapterCapabilities;

    fn commit_write_plan_async(
        &self,
        plan: ReadModelWritePlan,
    ) -> impl Future<Output = Result<ReadModelCommitOutcome, ReadModelError>> + Send + '_;

    fn is_processed_async<'a>(
        &'a self,
        consumer_name: &'a str,
        message_id: &'a str,
    ) -> impl Future<Output = Result<bool, ReadModelError>> + Send + 'a;
}

/// Async primary-key relational read-model query contract.
pub trait AsyncRelationalReadModelQueryStore: Send + Sync {
    fn read_model_query_capabilities_async(&self) -> ReadModelQueryCapabilities;

    fn load_graph_async(
        &self,
        request: ReadModelLoadRequest,
    ) -> impl Future<Output = Result<ReadModelLoadGraph, ReadModelError>> + Send + '_;
}

/// Async snapshot persistence keyed by full stream identity.
pub trait AsyncSnapshotStore: Send + Sync {
    fn get_snapshot_async<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<Option<SnapshotRecord>, RepositoryError>> + Send + 'a;

    fn save_snapshot_async<'a>(
        &'a self,
        identity: &'a StreamIdentity,
        record: SnapshotRecord,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a;

    fn delete_snapshot_async<'a>(
        &'a self,
        identity: &'a StreamIdentity,
    ) -> impl Future<Output = Result<bool, RepositoryError>> + Send + 'a;
}

/// Async worker-facing outbox repository operations.
pub trait AsyncOutboxRepositoryExt: Send + Sync {
    fn outbox_messages_by_status_async(
        &self,
        status: OutboxMessageStatus,
    ) -> impl Future<Output = Result<Vec<OutboxMessage>, RepositoryError>> + Send + '_;

    fn outbox_messages_pending_async(
        &self,
    ) -> impl Future<Output = Result<Vec<OutboxMessage>, RepositoryError>> + Send + '_ {
        async move {
            self.outbox_messages_by_status_async(OutboxMessageStatus::Pending)
                .await
        }
    }

    fn claim_outbox_messages_async<'a>(
        &'a self,
        worker_id: &'a str,
        max: usize,
        lease: Duration,
    ) -> impl Future<Output = Result<Vec<OutboxMessage>, RepositoryError>> + Send + 'a;

    fn complete_outbox_message_for_worker_async<'a>(
        &'a self,
        message_id: &'a str,
        worker_id: &'a str,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a;

    fn release_outbox_message_for_worker_async<'a>(
        &'a self,
        message_id: &'a str,
        worker_id: &'a str,
        error: &'a str,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a;

    fn fail_outbox_message_for_worker_async<'a>(
        &'a self,
        message_id: &'a str,
        worker_id: &'a str,
        error: &'a str,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a;

    fn record_outbox_publish_failure_async<'a>(
        &'a self,
        message_id: &'a str,
        worker_id: &'a str,
        error: &'a str,
        max_attempts: u32,
    ) -> impl Future<Output = Result<OutboxPublishFailureAction, RepositoryError>> + Send + 'a;
}
