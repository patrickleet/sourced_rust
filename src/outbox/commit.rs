use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::aggregate::{Aggregate, AggregateRepository};
use crate::outbox::OutboxMessage;
use crate::repository::{
    CommitBatch, RepositoryError, StreamIdentity, StreamWrite, TransactionalCommit,
};

/// Publishes an already-committed, claimed outbox row and settles its claim.
///
/// Implemented by the outbox → bus bridge and installed on an
/// [`AggregateRepository`] (by `Service::with_bus`) so that
/// `repo.outbox(msg).commit(agg)` publishes immediately — no separate call. The
/// hook owns the publisher and the outbox store; it is given the claimed message
/// the commit just wrote, publishes it, and completes the claim (or releases it
/// for the polling worker on failure). It is object-safe so the repository can
/// hold it without naming the transport/store types.
pub trait OutboxPublishHook: Send + Sync {
    /// Publish a committed, claimed outbox row and settle its claim. Publish
    /// failures are absorbed (the row stays retryable for the worker); only a
    /// store error surfaces.
    fn publish_claimed<'a>(
        &'a self,
        claimed: OutboxMessage,
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

/// Helper returned by [`AggregateRepository::outbox`] to commit an aggregate
/// and an outbox row in the same async transactional batch.
///
/// Borrows the repository so it can be called through `ctx.repo()` inside async
/// handlers.
pub struct OutboxCommit<'a, R, A> {
    repo: &'a AggregateRepository<R, A>,
    message: OutboxMessage,
}

impl<R, A> OutboxCommit<'_, R, A>
where
    R: TransactionalCommit,
    A: Aggregate + Send,
{
    /// Commit the aggregate and outbox message together, and — when the
    /// repository has a bus configured (via `Service::with_bus`) — publish the
    /// row immediately.
    ///
    /// With a bus configured, the row is **claimed in this same transaction**
    /// (born `InFlight` under a short lease) and published right after commit, so
    /// publication needs no separate claim and cannot race the polling worker; a
    /// crash before publish hands the row back to the worker at lease expiry, and
    /// a publish failure leaves it retryable. Without a bus, the row is committed
    /// `pending` for the polling worker to publish. A snapshot is also staged when
    /// the repository has snapshots configured and one is due — all in the one
    /// transaction.
    ///
    /// Returns a [`CommitReceipt`] carrying the inserted outbox message id.
    pub async fn commit(mut self, aggregate: &mut A) -> Result<CommitReceipt, RepositoryError> {
        self.message.set_source(aggregate);
        let outbox_message_id = self.message.id().to_string();

        // When a bus is configured, claim the row in this transaction so it can
        // be published immediately after commit; otherwise leave it `pending`.
        let publisher = self.repo.outbox_publisher();
        let claimed = match publisher {
            Some(config) => {
                self.message
                    .claim_at(&config.worker_id, config.lease, SystemTime::now())?;
                Some(self.message.clone())
            }
            None => None,
        };

        let (snapshots, snapshot_version) = self.repo.snapshot_writes_for(aggregate)?;
        let identity = StreamIdentity::new(A::aggregate_type(), aggregate.entity().id())?;
        let stream = StreamWrite::new(identity, aggregate.entity_mut());
        let mut batch = CommitBatch::new(vec![stream]);
        batch.outbox_messages.push(self.message);
        batch.snapshots = snapshots;
        self.repo.repo().commit_batch(batch).await?;
        if let Some(version) = snapshot_version {
            aggregate.entity_mut().set_snapshot_version(version);
        }

        // Best-effort immediate publish. A failure leaves the claimed row for the
        // polling worker and never fails the already-committed command.
        if let (Some(config), Some(claimed)) = (publisher, claimed) {
            let _ = config.hook.publish_claimed(claimed).await;
        }

        Ok(CommitReceipt {
            outbox_message_ids: vec![outbox_message_id],
        })
    }
}

impl<R, A> AggregateRepository<R, A> {
    /// Attach an outbox message to be committed with the aggregate.
    pub fn outbox(&self, message: OutboxMessage) -> OutboxCommit<'_, R, A> {
        OutboxCommit {
            repo: self,
            message,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{sourced, AggregateBuilder, Entity, HashMapRepository, OutboxStore};
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
        let repo = HashMapRepository::new().aggregate::<Dummy>();

        let mut aggregate = Dummy::default();
        aggregate.touch().unwrap();

        let event = OutboxMessage::create("msg-1", "DummyTouched", b"{}".to_vec()).unwrap();

        let receipt = repo.outbox(event).commit(&mut aggregate).await.unwrap();

        // The receipt reports the inserted outbox row so an after-commit
        // dispatcher knows what to publish.
        assert!(receipt.has_outbox_messages());
        assert_eq!(receipt.outbox_message_ids(), ["msg-1".to_string()]);

        let pending = repo.repo().outbox_store().pending().unwrap();
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

        assert_eq!(err, RepositoryError::Model("outbox write failed".into()));
        assert_eq!(aggregate.entity.committed_version(), 0);
        assert_eq!(aggregate.entity.new_events().len(), 1);
        assert_eq!(
            repo.repo().seen_ids.lock().unwrap().as_slice(),
            &["dummy-1".to_string(), "msg-fail".to_string()]
        );
    }
}
