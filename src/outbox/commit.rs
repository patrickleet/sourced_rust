use crate::aggregate::{Aggregate, AggregateRepository};
use crate::outbox::OutboxMessage;
use crate::repository::{
    CommitBatch, RepositoryError, StreamIdentity, StreamWrite, TransactionalCommit,
};

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
    pub async fn commit(mut self, aggregate: &mut A) -> Result<(), RepositoryError> {
        self.message.set_source(aggregate);
        let identity = StreamIdentity::new(A::aggregate_type(), aggregate.entity().id())?;
        let stream = StreamWrite::new(identity, aggregate.entity_mut());
        let mut batch = CommitBatch::new(vec![stream]);
        batch.outbox_messages.push(self.message);
        self.repo.repo().commit_batch(batch).await
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

        repo.outbox(event).commit(&mut aggregate).await.unwrap();

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
