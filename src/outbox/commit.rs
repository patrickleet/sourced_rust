use std::time::{Duration, SystemTime};

use crate::aggregate::{Aggregate, AggregateRepository};
use crate::outbox::OutboxMessage;
use crate::repository::{
    CommitBatch, RepositoryError, StreamIdentity, StreamWrite, TransactionalCommit,
};

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
    /// Commit the aggregate and outbox message together.
    ///
    /// Returns a [`CommitReceipt`] carrying the inserted outbox message id, so
    /// an after-commit dispatcher can publish exactly the rows this transaction
    /// wrote without re-scanning the outbox.
    pub async fn commit(mut self, aggregate: &mut A) -> Result<CommitReceipt, RepositoryError> {
        self.message.set_source(aggregate);
        let outbox_message_id = self.message.id().to_string();
        // Stage a snapshot too when the repository has snapshots configured and
        // one is due — same transaction as the events and the outbox row.
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
        Ok(CommitReceipt {
            outbox_message_ids: vec![outbox_message_id],
        })
    }

    /// Commit the aggregate and outbox message together, **claiming the outbox
    /// row for publication in the same transaction**.
    ///
    /// The row inserts already `InFlight` under `worker_id`'s lease
    /// (`attempts = 1`), so the caller can publish it immediately after commit
    /// without a separate claim and without racing the polling worker. While the
    /// lease is held the poller skips the row; if the caller never publishes
    /// (e.g. a crash), the lease expires and the worker reclaims it.
    ///
    /// Returns the receipt plus a clone of the claimed message so the caller can
    /// build the transport message and settle the claim (`complete` on success,
    /// `record_failure` on a publish error). The publish itself happens after
    /// this returns — a broker call is never held open inside the transaction.
    pub async fn commit_claimed(
        mut self,
        aggregate: &mut A,
        worker_id: &str,
        lease: Duration,
    ) -> Result<(CommitReceipt, OutboxMessage), RepositoryError> {
        self.message.set_source(aggregate);
        self.message.claim_at(worker_id, lease, SystemTime::now())?;
        let claimed = self.message.clone();
        let outbox_message_id = self.message.id().to_string();
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
        Ok((
            CommitReceipt {
                outbox_message_ids: vec![outbox_message_id],
            },
            claimed,
        ))
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

/// A repository that can commit an aggregate together with an outbox message in
/// one transaction, staging whatever else the repository requires (for example a
/// snapshot, for snapshot-backed repositories).
///
/// This is the abstraction
/// [`Context::commit_outbox`](crate::microsvc::Context::commit_outbox) binds to,
/// so the durable-enqueue command path works for **any** repository shape — a
/// plain [`AggregateRepository`] or a `SnapshotAggregateRepository` — not just
/// one. The underlying `CommitBatch`/`TransactionalCommit` boundary already
/// applies streams, outbox rows, read models, and snapshots in one transaction;
/// this trait exposes that to the ergonomic command path.
pub trait OutboxCommitting<A> {
    /// Commit the aggregate and outbox row (left `pending`) in one transaction.
    /// The polling worker publishes the row later.
    fn commit_outbox_pending(
        &self,
        aggregate: &mut A,
        message: OutboxMessage,
    ) -> impl core::future::Future<Output = Result<CommitReceipt, RepositoryError>> + Send;

    /// Commit the aggregate and outbox row, claiming the row in the same
    /// transaction for immediate publication. Returns a clone of the claimed
    /// message so the caller can build the transport message and settle the claim.
    fn commit_outbox_claimed(
        &self,
        aggregate: &mut A,
        message: OutboxMessage,
        worker_id: &str,
        lease: Duration,
    ) -> impl core::future::Future<Output = Result<(CommitReceipt, OutboxMessage), RepositoryError>> + Send;
}

impl<R, A> OutboxCommitting<A> for AggregateRepository<R, A>
where
    R: TransactionalCommit + Send + Sync,
    A: Aggregate + Send + Sync,
{
    async fn commit_outbox_pending(
        &self,
        aggregate: &mut A,
        message: OutboxMessage,
    ) -> Result<CommitReceipt, RepositoryError> {
        self.outbox(message).commit(aggregate).await
    }

    async fn commit_outbox_claimed(
        &self,
        aggregate: &mut A,
        message: OutboxMessage,
        worker_id: &str,
        lease: Duration,
    ) -> Result<(CommitReceipt, OutboxMessage), RepositoryError> {
        self.outbox(message)
            .commit_claimed(aggregate, worker_id, lease)
            .await
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
    async fn commit_claimed_inserts_row_in_flight_under_lease() {
        let repo = HashMapRepository::new().aggregate::<Dummy>();

        let mut aggregate = Dummy::default();
        aggregate.touch().unwrap();

        let event = OutboxMessage::create("msg-1", "DummyTouched", b"{}".to_vec()).unwrap();

        let (receipt, claimed) = repo
            .outbox(event)
            .commit_claimed(&mut aggregate, "immediate:test", Duration::from_secs(5))
            .await
            .unwrap();

        // The committed row is claimed in the same transaction: in-flight, under
        // this worker's lease, attempt 1.
        assert_eq!(receipt.outbox_message_ids(), ["msg-1".to_string()]);
        assert!(!claimed.is_pending());
        assert_eq!(claimed.attempts, 1);

        // A polling worker sees nothing claimable while the lease is held.
        let pending = repo.repo().outbox_store().pending().unwrap();
        assert!(
            pending.is_empty(),
            "a row claimed in-transaction must not be claimable by the poller"
        );
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
