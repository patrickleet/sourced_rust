use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::aggregate::{Aggregate, AggregateRepository};
use crate::outbox::OutboxMessage;
use crate::read_model::ReadModelWritePlanBuilder;
use crate::repository::{
    CommitBatch, RepositoryError, StreamIdentity, StreamWrite, TransactionalCommit,
};
use crate::table::TableWritePlan;

/// Publishes already-committed, claimed outbox rows and settles their claims.
///
/// Implemented by the outbox → bus bridge and installed on an
/// [`AggregateRepository`] (by `Service::with_bus`) so that
/// `repo.outbox(msg).commit(agg)` publishes immediately — no separate call. The
/// hook owns the publisher and the outbox store; it is given the claimed
/// messages the commit just wrote (in insertion order), publishes them, and
/// completes each claim (or releases it for the polling worker on failure). It
/// is object-safe so the repository can hold it without naming the
/// transport/store types.
pub trait OutboxPublishHook: Send + Sync {
    /// Publish committed, claimed outbox rows and settle their claims. Publish
    /// failures are absorbed (the rows stay retryable for the worker); only a
    /// store error surfaces.
    fn publish_claimed<'a>(
        &'a self,
        claimed: Vec<OutboxMessage>,
    ) -> Pin<Box<dyn Future<Output = Result<(), RepositoryError>> + Send + 'a>>;
}

/// Outbox publisher installed on a repository so commits publish immediately.
pub struct OutboxPublisherConfig {
    pub(crate) hook: Arc<dyn OutboxPublishHook>,
    pub(crate) worker_id: String,
    pub(crate) lease: Duration,
}

impl OutboxPublisherConfig {
    /// Build the config from a publish hook, the worker id used to scope the
    /// in-transaction claim, and the publish lease.
    pub fn new(
        hook: Arc<dyn OutboxPublishHook>,
        worker_id: impl Into<String>,
        lease: Duration,
    ) -> Self {
        Self {
            hook,
            worker_id: worker_id.into(),
            lease,
        }
    }
}

/// Outcome of an outbox-bearing commit.
///
/// Carries the ids of the outbox rows the transaction inserted so an
/// after-commit dispatcher knows exactly which rows to publish. This is the seam
/// the immediate-dispatch path hangs off (see
/// `specs/durable-enqueue-outbox-dispatch`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommitReceipt {
    /// Ids of the outbox messages inserted by this commit, in insertion order.
    pub outbox_message_ids: Vec<String>,
}

impl CommitReceipt {
    /// The outbox message ids inserted by this commit.
    pub fn outbox_message_ids(&self) -> &[String] {
        &self.outbox_message_ids
    }

    /// Whether this commit inserted any outbox messages.
    pub fn has_outbox_messages(&self) -> bool {
        !self.outbox_message_ids.is_empty()
    }
}

/// Builder returned by [`AggregateRepository::outbox`] and
/// [`AggregateRepository::read_models`] that commits an aggregate together with
/// outbox rows, relational read-model writes, and (when the repository has
/// snapshots configured) a snapshot — all in one async transactional batch.
///
/// Borrows the repository so it can be called through `ctx.repo()` inside async
/// handlers. Chain `.outbox(..)` / `.read_models(..)` to stage more, then
/// `.commit(&mut aggregate)`.
pub struct AggregateCommit<'a, R, A> {
    repo: &'a AggregateRepository<R, A>,
    outbox_messages: Vec<OutboxMessage>,
    read_model_plans: Vec<TableWritePlan>,
    error: Option<RepositoryError>,
}

impl<'a, R, A> AggregateCommit<'a, R, A> {
    fn empty(repo: &'a AggregateRepository<R, A>) -> Self {
        Self {
            repo,
            outbox_messages: Vec::new(),
            read_model_plans: Vec::new(),
            error: None,
        }
    }

    /// Stage an outbox message to publish/enqueue with the commit.
    pub fn outbox(mut self, message: OutboxMessage) -> Self {
        self.outbox_messages.push(message);
        self
    }

    /// Stage relational read-model writes to apply in the same transaction.
    pub fn read_models(mut self, read_models: ReadModelWritePlanBuilder) -> Self {
        if self.error.is_none() {
            match read_models.into_write_plan() {
                Ok(plan) => self.read_model_plans.push(plan),
                Err(err) => self.error = Some(err.into()),
            }
        }
        self
    }
}

impl<R, A> AggregateCommit<'_, R, A>
where
    R: TransactionalCommit,
    A: Aggregate + Send,
{
    /// Commit the aggregate together with the staged outbox rows, read-model
    /// writes, and a snapshot (when due) in one transaction — and, when the
    /// repository has a bus configured (via `Service::with_bus`), publish the
    /// outbox rows immediately.
    ///
    /// With a bus configured, each outbox row is **claimed in this same
    /// transaction** (born `InFlight` under a short lease) and published right
    /// after commit, so publication needs no separate claim and cannot race the
    /// polling worker; a crash before publish hands the row back to an
    /// independently running worker at lease expiry, and a publish failure
    /// leaves it retryable. Without a bus, rows are committed `pending` for the
    /// worker to publish.
    ///
    /// Returns a [`CommitReceipt`] carrying the inserted outbox message ids.
    pub async fn commit(mut self, aggregate: &mut A) -> Result<CommitReceipt, RepositoryError> {
        if let Some(err) = self.error.take() {
            return Err(err);
        }
        for message in &mut self.outbox_messages {
            message.set_source(aggregate);
        }
        let outbox_message_ids: Vec<String> = self
            .outbox_messages
            .iter()
            .map(|message| message.id().to_string())
            .collect();

        // When a bus is configured, claim the rows in this transaction so they
        // can be published immediately after commit; otherwise leave them
        // `pending`.
        let publisher = self.repo.outbox_publisher();
        let mut claimed = Vec::new();
        if let Some(config) = publisher {
            let now = SystemTime::now();
            for message in &mut self.outbox_messages {
                message.claim_at(&config.worker_id, config.lease, now)?;
                claimed.push(message.clone());
            }
        }

        let (snapshots, snapshot_version) = self.repo.snapshot_writes_for(aggregate)?;
        let identity = StreamIdentity::new(A::aggregate_type(), aggregate.entity().id())?;
        let stream = StreamWrite::new(identity, aggregate.entity_mut());
        self.repo
            .repo()
            .commit_batch(CommitBatch {
                streams: vec![stream],
                outbox_messages: self.outbox_messages,
                read_model_plans: self.read_model_plans,
                snapshots,
                inbox_receipts: Vec::new(),
            })
            .await?;
        if let Some(version) = snapshot_version {
            aggregate.entity_mut().set_snapshot_version(version);
        }

        // Best-effort immediate publish. A failure leaves the claimed rows for
        // an independently running polling worker and never fails the
        // already-committed command.
        if let Some(config) = publisher {
            let _ = config.hook.publish_claimed(claimed).await;
        }

        Ok(CommitReceipt { outbox_message_ids })
    }
}

impl<R, A> AggregateRepository<R, A> {
    /// Start a commit with an outbox message attached.
    pub fn outbox(&self, message: OutboxMessage) -> AggregateCommit<'_, R, A> {
        AggregateCommit::empty(self).outbox(message)
    }

    /// Start a commit with relational read-model writes attached. Composes with
    /// `.outbox(..)`, the aggregate's events, and snapshots in one transaction.
    pub fn read_models(&self, read_models: ReadModelWritePlanBuilder) -> AggregateCommit<'_, R, A> {
        AggregateCommit::empty(self).read_models(read_models)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{sourced, AggregateBuilder, Entity, InMemoryRepository, OutboxStore};
    use std::sync::Mutex;

    #[derive(Default)]
    struct Dummy {
        entity: Entity,
    }

    #[sourced(entity)]
    impl Dummy {
        #[event("Touched")]
        fn touch(&mut self) {
            if self.entity.id().is_empty() {
                self.entity.set_id("dummy-1");
            }
        }
    }

    #[derive(Default)]
    struct FailingOutboxRepo {
        seen_ids: Mutex<Vec<String>>,
    }

    impl TransactionalCommit for FailingOutboxRepo {
        async fn commit_batch<'a>(&'a self, batch: CommitBatch<'a>) -> Result<(), RepositoryError> {
            {
                *self.seen_ids.lock().unwrap() = batch
                    .streams
                    .iter()
                    .map(|stream| stream.entity.id().to_string())
                    .chain(
                        batch
                            .outbox_messages
                            .iter()
                            .map(|message| message.id().to_string()),
                    )
                    .collect();

                Err(RepositoryError::Model("outbox write failed".into()))
            }
        }
    }

    #[tokio::test]
    async fn outbox_helper_commits_both_entities() {
        let repo = InMemoryRepository::new().aggregate::<Dummy>();

        let mut aggregate = Dummy::default();
        aggregate.touch().unwrap();

        let event = OutboxMessage::create("msg-1", "DummyTouched", b"{}".to_vec()).unwrap();

        let receipt = repo.outbox(event).commit(&mut aggregate).await.unwrap();

        // The receipt reports the inserted outbox row so an after-commit
        // dispatcher knows what to publish.
        assert!(receipt.has_outbox_messages());
        assert_eq!(receipt.outbox_message_ids(), ["msg-1".to_string()]);

        let pending = repo
            .repo()
            .outbox_store()
            .pending(usize::MAX)
            .await
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id(), "msg-1");
    }

    #[tokio::test]
    async fn outbox_helper_failure_leaves_entities_uncommitted() {
        let repo = AggregateRepository::<_, Dummy>::new(FailingOutboxRepo::default());

        let mut aggregate = Dummy::default();
        aggregate.touch().unwrap();

        let event = OutboxMessage::create("msg-fail", "DummyTouched", b"{}".to_vec()).unwrap();

        let err = repo.outbox(event).commit(&mut aggregate).await.unwrap_err();

        assert!(
            matches!(&err, RepositoryError::Model(message) if message == "outbox write failed"),
            "unexpected error: {err}"
        );
        assert_eq!(aggregate.entity.committed_version(), 0);
        assert_eq!(aggregate.entity.new_events().len(), 1);
        assert_eq!(
            repo.repo().seen_ids.lock().unwrap().as_slice(),
            &["dummy-1".to_string(), "msg-fail".to_string()]
        );
    }

    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize, crate::ReadModel)]
    #[table("agg_commit_views")]
    struct ComposeView {
        #[id]
        id: String,
        n: i32,
    }

    #[derive(Default, crate::Snapshot)]
    struct ComposeCounter {
        entity: Entity,
        value: i64,
    }

    #[sourced(entity, aggregate_type = "agg_commit_counter")]
    impl ComposeCounter {
        #[event("bumped")]
        fn bump(&mut self, id: String) {
            self.entity.set_id(&id);
            self.value += 1;
        }
    }

    #[tokio::test]
    async fn read_models_and_snapshot_commit_in_one_transaction() {
        use crate::{
            Aggregate, ReadModelWorkspaceExt, ReadModelWritePlanBuilder, RowKey, RowValue,
            SnapshotStore, StreamIdentity,
        };

        let repo = InMemoryRepository::new()
            .aggregate::<ComposeCounter>()
            .with_snapshots(1);

        let mut counter = ComposeCounter::default();
        counter.bump("c1".to_string()).unwrap();

        let mut plan = ReadModelWritePlanBuilder::new();
        plan.upsert(&ComposeView {
            id: "c1".into(),
            n: 1,
        })
        .unwrap();

        // Read-model writes + the aggregate's events + a snapshot — one commit.
        repo.read_models(plan).commit(&mut counter).await.unwrap();

        // The read-model row is committed...
        let loaded = repo
            .repo()
            .model_store()
            .workspace()
            .load::<ComposeView>(RowKey::new([("id", RowValue::String("c1".into()))]))
            .one()
            .await
            .unwrap();
        assert!(loaded.is_some(), "read-model row should be committed");

        // ...and a snapshot was staged in the same transaction (frequency 1).
        let identity = StreamIdentity::new(ComposeCounter::aggregate_type(), "c1").unwrap();
        let snapshot = repo.repo().get_snapshot(&identity).await.unwrap();
        assert!(
            snapshot.is_some(),
            "snapshot should be staged alongside the read-model commit"
        );
    }

    #[tokio::test]
    async fn aggregate_outbox_read_model_and_snapshot_commit_in_one_transaction() {
        use crate::{
            Aggregate, GetStream, ReadModelWorkspaceExt, ReadModelWritePlanBuilder, RowKey,
            RowValue, SnapshotStore, StreamIdentity,
        };

        let repo = InMemoryRepository::new()
            .aggregate::<ComposeCounter>()
            .with_snapshots(1);

        let mut counter = ComposeCounter::default();
        counter.bump("c1".to_string()).unwrap();

        let mut plan = ReadModelWritePlanBuilder::new();
        plan.upsert(&ComposeView {
            id: "c1".into(),
            n: 1,
        })
        .unwrap();

        let message = OutboxMessage::create("evt-c1", "counter.bumped", b"{}".to_vec()).unwrap();

        // All four in one commit: aggregate events + outbox row + read-model
        // write + snapshot.
        let receipt = repo
            .outbox(message)
            .read_models(plan)
            .commit(&mut counter)
            .await
            .unwrap();
        assert_eq!(receipt.outbox_message_ids(), ["evt-c1".to_string()]);

        let identity = StreamIdentity::new(ComposeCounter::aggregate_type(), "c1").unwrap();

        // 1) aggregate stream committed
        assert!(
            repo.repo().get_stream(&identity).await.unwrap().is_some(),
            "aggregate stream should be committed"
        );

        // 2) outbox row present (pending — no bus attached here)
        let pending = repo
            .repo()
            .outbox_store()
            .pending(usize::MAX)
            .await
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id(), "evt-c1");

        // 3) read-model row written
        let loaded = repo
            .repo()
            .model_store()
            .workspace()
            .load::<ComposeView>(RowKey::new([("id", RowValue::String("c1".into()))]))
            .one()
            .await
            .unwrap();
        assert!(loaded.is_some(), "read-model row should be committed");

        // 4) snapshot staged
        assert!(
            repo.repo().get_snapshot(&identity).await.unwrap().is_some(),
            "snapshot should be staged"
        );
    }
}
