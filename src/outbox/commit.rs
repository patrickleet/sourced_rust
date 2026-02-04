use crate::core::{Aggregate, AggregateRepository, Repository, RepositoryError};
use crate::outbox::OutboxMessage;

/// Helper returned by [`OutboxCommitExt::outbox`] to commit an aggregate and outbox
/// message in the same repository commit.
pub struct OutboxCommit<'a, R, A> {
    repo: &'a AggregateRepository<R, A>,
    event: &'a mut OutboxMessage,
}

impl<'a, R, A> OutboxCommit<'a, R, A>
where
    R: Repository,
    A: Aggregate,
{
    /// Commit the aggregate and outbox message together.
    pub fn commit(self, aggregate: &mut A) -> Result<(), RepositoryError> {
        let mut entities = [aggregate.entity_mut(), self.event.entity_mut()];
        self.repo.repo().commit(&mut entities)
    }
}

/// Extension trait for aggregate repositories to commit outbox messages alongside aggregates.
pub trait OutboxCommitExt<R, A>
where
    R: Repository,
    A: Aggregate,
{
    /// Attach an outbox message to be committed with the aggregate.
    fn outbox<'a>(&'a self, event: &'a mut OutboxMessage) -> OutboxCommit<'a, R, A>;
}

impl<R, A> OutboxCommitExt<R, A> for AggregateRepository<R, A>
where
    R: Repository,
    A: Aggregate,
{
    fn outbox<'a>(&'a self, event: &'a mut OutboxMessage) -> OutboxCommit<'a, R, A> {
        OutboxCommit { repo: self, event }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{impl_aggregate, AggregateBuilder, Entity, EventRecord, HashMapRepository};

    #[derive(Default)]
    struct Dummy {
        entity: Entity,
    }

    impl Dummy {
        fn touch(&mut self) {
            if self.entity.id().is_empty() {
                self.entity.set_id("dummy-1");
            }
            self.entity.digest("Touched", Vec::new());
        }

        fn replay(&mut self, _event: &EventRecord) -> Result<(), String> {
            Ok(())
        }
    }

    impl_aggregate!(Dummy, entity, replay);

    #[test]
    fn outbox_helper_commits_both_entities() {
        let repo = HashMapRepository::new().aggregate::<Dummy>();

        let mut aggregate = Dummy::default();
        aggregate.touch();

        let mut event = OutboxMessage::new("msg-1", "DummyTouched", "{}");

        repo.outbox(&mut event).commit(&mut aggregate).unwrap();

        let stored_agg = repo.repo().get("dummy-1").unwrap();
        assert!(stored_agg.is_some());

        let stored_event = repo.repo().get(event.id()).unwrap();
        assert!(stored_event.is_some());
    }
}
