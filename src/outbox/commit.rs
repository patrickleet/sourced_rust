use crate::aggregate::{Aggregate, AggregateRepository, AsyncAggregateRepository};
use crate::outbox::OutboxMessage;
use crate::repository::{
    AsyncCommitBatch, AsyncStreamWrite, AsyncTransactionalCommit, CommitBatch, RepositoryError,
    StreamIdentity, TransactionalCommit,
};

/// Helper returned by [`SyncOutboxCommitExt::outbox_sync`] to commit an aggregate and outbox
/// message in the same transactional commit batch.
pub struct SyncOutboxCommit<'a, R, A> {
    repo: &'a AggregateRepository<R, A>,
    message: OutboxMessage,
}

impl<'a, R, A> SyncOutboxCommit<'a, R, A>
where
    R: TransactionalCommit,
    A: Aggregate,
{
    /// Commit the aggregate and outbox message together.
    pub fn commit_sync(mut self, aggregate: &mut A) -> Result<(), RepositoryError> {
        self.message.set_source(aggregate);
        let mut batch = CommitBatch::new(vec![aggregate.entity_mut()]);
        batch.outbox_messages.push(self.message);
        self.repo.repo().commit_batch(batch)
    }
}

/// Extension trait for aggregate repositories to commit outbox messages alongside aggregates.
pub trait SyncOutboxCommitExt<R, A>
where
    R: TransactionalCommit,
    A: Aggregate,
{
    /// Attach an outbox message to be committed with the aggregate.
    fn outbox_sync<'a>(&'a self, message: OutboxMessage) -> SyncOutboxCommit<'a, R, A>;
}

impl<R, A> SyncOutboxCommitExt<R, A> for AggregateRepository<R, A>
where
    R: TransactionalCommit,
    A: Aggregate,
{
    fn outbox_sync<'a>(&'a self, message: OutboxMessage) -> SyncOutboxCommit<'a, R, A> {
        SyncOutboxCommit {
            repo: self,
            message,
        }
    }
}

/// Helper returned by [`AsyncAggregateRepository::outbox`] to commit an aggregate
/// and an outbox row in the same async transactional batch.
///
/// Borrows the repository (mirroring the synchronous [`outbox_sync`](AsyncOutboxCommitExt))
/// so it can be called through `ctx.repo()` inside async handlers.
pub struct AsyncOutboxCommit<'a, R, A> {
    repo: &'a AsyncAggregateRepository<R, A>,
    message: OutboxMessage,
}

impl<R, A> AsyncOutboxCommit<'_, R, A>
where
    R: AsyncTransactionalCommit,
    A: Aggregate + Send,
{
    /// Commit the aggregate and outbox message together.
    pub async fn commit(mut self, aggregate: &mut A) -> Result<(), RepositoryError> {
        self.message.set_source(aggregate);
        let identity = StreamIdentity::new(A::aggregate_type(), aggregate.entity().id())?;
        let stream = AsyncStreamWrite::new(identity, aggregate.entity_mut());
        let mut batch = AsyncCommitBatch::new(vec![stream]);
        batch.outbox_messages.push(self.message);
        self.repo.repo().commit_batch_async(batch).await
    }
}

impl<R, A> AsyncAggregateRepository<R, A> {
    /// Attach an outbox message to be committed with the aggregate.
    pub fn outbox(&self, message: OutboxMessage) -> AsyncOutboxCommit<'_, R, A> {
        AsyncOutboxCommit {
            repo: self,
            message,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        sourced, AggregateBuilder, CommitBatch, Entity, HashMapRepository, OutboxStore,
        TransactionalCommit,
    };
    use std::cell::RefCell;

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
        seen_ids: RefCell<Vec<String>>,
    }

    impl TransactionalCommit for FailingOutboxRepo {
        fn commit_batch(&self, batch: CommitBatch<'_>) -> Result<(), RepositoryError> {
            *self.seen_ids.borrow_mut() = batch
                .entities
                .iter()
                .map(|entity| entity.id().to_string())
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

    #[test]
    fn outbox_helper_commits_both_entities() {
        let repo = HashMapRepository::new().aggregate::<Dummy>();

        let mut aggregate = Dummy::default();
        aggregate.touch().unwrap();

        let event = OutboxMessage::create("msg-1", "DummyTouched", b"{}".to_vec()).unwrap();

        repo.outbox_sync(event).commit_sync(&mut aggregate).unwrap();

        let pending = repo.repo().outbox_store().pending().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id(), "msg-1");
    }

    #[test]
    fn outbox_helper_failure_leaves_entities_uncommitted() {
        let repo = AggregateRepository::<_, Dummy>::new(FailingOutboxRepo::default());

        let mut aggregate = Dummy::default();
        aggregate.touch().unwrap();

        let event = OutboxMessage::create("msg-fail", "DummyTouched", b"{}".to_vec()).unwrap();

        let err = repo
            .outbox_sync(event)
            .commit_sync(&mut aggregate)
            .unwrap_err();

        assert_eq!(err, RepositoryError::Model("outbox write failed".into()));
        assert_eq!(aggregate.entity.committed_version(), 0);
        assert_eq!(aggregate.entity.new_events().len(), 1);
        assert_eq!(
            repo.repo().seen_ids.borrow().as_slice(),
            &["dummy-1".to_string(), "msg-fail".to_string()]
        );
    }
}
