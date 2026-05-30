use crate::aggregate::{Aggregate, AsyncAggregateRepository};
use crate::outbox::OutboxMessage;
use crate::repository::{
    AsyncCommitBatch, AsyncStreamWrite, AsyncTransactionalCommit, RepositoryError, StreamIdentity,
};

/// Helper returned by [`AsyncAggregateRepository::outbox`] to commit an aggregate
/// and an outbox row in the same async transactional batch.
///
/// Borrows the repository so it can be called through `ctx.repo()` inside async
/// handlers.
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
    use crate::{sourced, AsyncAggregateBuilder, Entity, HashMapRepository, OutboxStore};
    use std::sync::Mutex;

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        use std::ptr;
        use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
        const VTABLE: RawWakerVTable = RawWakerVTable::new(
            |_| RawWaker::new(ptr::null(), &VTABLE),
            |_| {},
            |_| {},
            |_| {},
        );
        let waker = unsafe { Waker::from_raw(RawWaker::new(ptr::null(), &VTABLE)) };
        let mut cx = Context::from_waker(&waker);
        let mut future = std::pin::pin!(future);
        loop {
            if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
                return output;
            }
        }
    }

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

    impl AsyncTransactionalCommit for FailingOutboxRepo {
        async fn commit_batch_async<'a>(
            &'a self,
            batch: AsyncCommitBatch<'a>,
        ) -> Result<(), RepositoryError> {
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

    #[test]
    fn outbox_helper_commits_both_entities() {
        let repo = HashMapRepository::new().async_aggregate::<Dummy>();

        let mut aggregate = Dummy::default();
        aggregate.touch().unwrap();

        let event = OutboxMessage::create("msg-1", "DummyTouched", b"{}".to_vec()).unwrap();

        block_on(repo.outbox(event).commit(&mut aggregate)).unwrap();

        let pending = repo.repo().outbox_store().pending().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id(), "msg-1");
    }

    #[test]
    fn outbox_helper_failure_leaves_entities_uncommitted() {
        let repo = AsyncAggregateRepository::<_, Dummy>::new(FailingOutboxRepo::default());

        let mut aggregate = Dummy::default();
        aggregate.touch().unwrap();

        let event = OutboxMessage::create("msg-fail", "DummyTouched", b"{}".to_vec()).unwrap();

        let err = block_on(repo.outbox(event).commit(&mut aggregate)).unwrap_err();

        assert_eq!(err, RepositoryError::Model("outbox write failed".into()));
        assert_eq!(aggregate.entity.committed_version(), 0);
        assert_eq!(aggregate.entity.new_events().len(), 1);
        assert_eq!(
            repo.repo().seen_ids.lock().unwrap().as_slice(),
            &["dummy-1".to_string(), "msg-fail".to_string()]
        );
    }
}
