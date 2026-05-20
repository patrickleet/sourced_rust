use crate::aggregate::{Aggregate, AggregateRepository};
use crate::outbox::OutboxMessage;
use crate::repository::{CommitBatch, RepositoryError, TransactionalCommit};

/// Helper returned by [`OutboxCommitExt::outbox`] to commit an aggregate and outbox
/// message in the same transactional commit batch.
pub struct OutboxCommit<'a, R, A> {
    repo: &'a AggregateRepository<R, A>,
    event: &'a mut OutboxMessage,
}

impl<'a, R, A> OutboxCommit<'a, R, A>
where
    R: TransactionalCommit,
    A: Aggregate,
{
    /// Commit the aggregate and outbox message together.
    pub fn commit(self, aggregate: &mut A) -> Result<(), RepositoryError> {
        self.repo.repo().commit_batch(CommitBatch::new(vec![
            aggregate.entity_mut(),
            self.event.entity_mut(),
        ]))
    }
}

/// Extension trait for aggregate repositories to commit outbox messages alongside aggregates.
pub trait OutboxCommitExt<R, A>
where
    R: TransactionalCommit,
    A: Aggregate,
{
    /// Attach an outbox message to be committed with the aggregate.
    fn outbox<'a>(&'a self, event: &'a mut OutboxMessage) -> OutboxCommit<'a, R, A>;
}

impl<R, A> OutboxCommitExt<R, A> for AggregateRepository<R, A>
where
    R: TransactionalCommit,
    A: Aggregate,
{
    fn outbox<'a>(&'a self, event: &'a mut OutboxMessage) -> OutboxCommit<'a, R, A> {
        OutboxCommit { repo: self, event }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        impl_aggregate, AggregateBuilder, CommitBatch, Entity, EventRecord, Get, HashMapRepository,
        TransactionalCommit,
    };
    use std::cell::RefCell;

    #[derive(Default)]
    struct Dummy {
        entity: Entity,
    }

    impl Dummy {
        fn touch(&mut self) {
            if self.entity.id().is_empty() {
                self.entity.set_id("dummy-1");
            }
            self.entity.digest_empty("Touched");
        }

        fn replay(&mut self, _event: &EventRecord) -> Result<(), String> {
            Ok(())
        }
    }

    impl_aggregate!(Dummy, entity, replay);

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
                .collect();

            Err(RepositoryError::Model("outbox write failed".into()))
        }
    }

    #[test]
    fn outbox_helper_commits_both_entities() {
        let repo = HashMapRepository::new().aggregate::<Dummy>();

        let mut aggregate = Dummy::default();
        aggregate.touch();

        let mut event = OutboxMessage::create("msg-1", "DummyTouched", b"{}".to_vec());

        repo.outbox(&mut event).commit(&mut aggregate).unwrap();

        let stored_agg = repo.repo().get("dummy-1").unwrap();
        assert!(stored_agg.is_some());

        let stored_event = repo.repo().get(event.id()).unwrap();
        assert!(stored_event.is_some());
    }

    #[test]
    fn outbox_helper_failure_leaves_entities_uncommitted() {
        let repo = AggregateRepository::<_, Dummy>::new(FailingOutboxRepo::default());

        let mut aggregate = Dummy::default();
        aggregate.touch();

        let mut event = OutboxMessage::create("msg-fail", "DummyTouched", b"{}".to_vec());

        let err = repo.outbox(&mut event).commit(&mut aggregate).unwrap_err();

        assert_eq!(err, RepositoryError::Model("outbox write failed".into()));
        assert_eq!(aggregate.entity.committed_version(), 0);
        assert_eq!(event.entity().committed_version(), 0);
        assert_eq!(aggregate.entity.new_events().len(), 1);
        assert_eq!(event.entity().new_events().len(), 1);
        assert_eq!(
            repo.repo().seen_ids.borrow().as_slice(),
            &["dummy-1".to_string(), "outbox:msg-fail".to_string()]
        );
    }
}
