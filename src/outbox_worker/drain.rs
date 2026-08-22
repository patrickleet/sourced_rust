//! Cancellable background drain over [`OutboxDispatcher::dispatch_batch`].
//!
//! Immediate after-commit publish stays the fast path. This loop is the
//! safety net: it only claims rows that are still pending (released after a
//! publish failure, or never claimed because the process crashed after
//! commit). It does not resurrect the old in-memory `OutboxWorker`.

use std::future::Future;
use std::time::Duration;

use tokio::task::JoinHandle;

use crate::bus::{MessagePublisher, TransportError};

use super::{OutboxDispatchOutcome, OutboxDispatcher, OutboxStore};

/// Default time between empty drain passes.
pub const DEFAULT_DRAIN_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Default claim batch for one drain pass.
pub const DEFAULT_DRAIN_BATCH_SIZE: usize = 32;

/// Default lease for drain claims. Longer than the immediate hook's 5s lease
/// so a pass can publish a real backlog without the row becoming claimable
/// by a second drainer.
pub const DEFAULT_DRAIN_LEASE: Duration = Duration::from_secs(30);

/// Default first backoff after a store error. Doubles up to
/// [`DEFAULT_DRAIN_MAX_ERROR_BACKOFF`].
pub const DEFAULT_DRAIN_ERROR_BACKOFF: Duration = Duration::from_secs(1);

/// Ceiling for store-error backoff.
pub const DEFAULT_DRAIN_MAX_ERROR_BACKOFF: Duration = Duration::from_secs(30);

/// Worker-id prefix that distinguishes drain claims from
/// `microsvc-immediate:<pid>`.
pub fn drain_worker_id() -> String {
    format!("drain:{}", std::process::id())
}

/// Repeatedly [`OutboxDispatcher::dispatch_batch`] until cancelled.
pub struct OutboxDrainRunner<S, P> {
    dispatcher: OutboxDispatcher<S, P>,
    batch_size: usize,
    poll_interval: Duration,
    error_backoff: Duration,
    max_error_backoff: Duration,
}

impl<S, P> OutboxDrainRunner<S, P>
where
    S: OutboxStore,
    P: MessagePublisher,
{
    /// Wrap an existing dispatcher. Reuse the dispatcher's lease / attempts /
    /// concurrency rather than duplicating them here.
    pub fn new(dispatcher: OutboxDispatcher<S, P>) -> Self {
        Self {
            dispatcher,
            batch_size: DEFAULT_DRAIN_BATCH_SIZE,
            poll_interval: DEFAULT_DRAIN_POLL_INTERVAL,
            error_backoff: DEFAULT_DRAIN_ERROR_BACKOFF,
            max_error_backoff: DEFAULT_DRAIN_MAX_ERROR_BACKOFF,
        }
    }

    /// Build a dispatcher + runner with the drain worker id and default lease.
    pub fn for_store(store: S, publisher: P, max_attempts: u32) -> Self {
        Self::new(OutboxDispatcher::new(
            store,
            publisher,
            drain_worker_id(),
            DEFAULT_DRAIN_LEASE,
            max_attempts,
        ))
    }

    pub fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.batch_size = batch_size.max(1);
        self
    }

    pub fn with_poll_interval(mut self, poll_interval: Duration) -> Self {
        self.poll_interval = poll_interval;
        self
    }

    pub fn with_error_backoff(mut self, error_backoff: Duration) -> Self {
        self.error_backoff = error_backoff;
        self
    }

    pub fn with_max_error_backoff(mut self, max_error_backoff: Duration) -> Self {
        self.max_error_backoff = max_error_backoff;
        self
    }

    /// The dispatcher this runner drives.
    pub fn dispatcher(&self) -> &OutboxDispatcher<S, P> {
        &self.dispatcher
    }

    /// Drain until `shutdown` resolves. Store errors back off; they do not
    /// terminate the loop. Empty passes sleep `poll_interval`. A full batch
    /// is followed immediately by another pass.
    pub async fn run(self, shutdown: impl Future<Output = ()>) -> Result<(), TransportError> {
        tokio::pin!(shutdown);
        let mut backoff = self.error_backoff;
        loop {
            let pass = async {
                match self.dispatcher.dispatch_batch(self.batch_size).await {
                    Ok(outcome) => Pass::Drained(outcome),
                    Err(error) => Pass::StoreError(error),
                }
            };
            tokio::select! {
                _ = &mut shutdown => return Ok(()),
                result = pass => match result {
                    Pass::Drained(outcome) => {
                        backoff = self.error_backoff;
                        if outcome.claimed < self.batch_size {
                            tokio::select! {
                                _ = &mut shutdown => return Ok(()),
                                _ = tokio::time::sleep(self.poll_interval) => {}
                            }
                        }
                    }
                    Pass::StoreError(error) => {
                        eprintln!("outbox drain: {error}");
                        tokio::select! {
                            _ = &mut shutdown => return Ok(()),
                            _ = tokio::time::sleep(backoff) => {}
                        }
                        backoff = backoff.saturating_mul(2).min(self.max_error_backoff);
                    }
                }
            }
        }
    }

    /// Spawn on the current tokio runtime. Dropping the handle does **not**
    /// stop the task (call [`OutboxDrainHandle::stop`]).
    pub fn spawn(self) -> OutboxDrainHandle
    where
        S: 'static,
        P: 'static,
    {
        let join = tokio::spawn(async move { self.run(std::future::pending()).await });
        OutboxDrainHandle { join }
    }
}

enum Pass {
    Drained(OutboxDispatchOutcome),
    StoreError(TransportError),
}

/// Handle for a spawned [`OutboxDrainRunner`]. Abort-only on [`stop`]; drop
/// leaves the task running so fire-and-forget hosts stay drained.
pub struct OutboxDrainHandle {
    join: JoinHandle<Result<(), TransportError>>,
}

impl OutboxDrainHandle {
    /// Abort the loop and wait for it to unwind.
    pub async fn stop(self) -> Result<(), TransportError> {
        self.join.abort();
        match self.join.await {
            Ok(result) => result,
            Err(error) if error.is_cancelled() => Ok(()),
            Err(error) => Err(TransportError::retryable(format!(
                "outbox drain task: {error}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::Message;
    use crate::outbox_worker::{ClaimOutboxMessages, OutboxClaimRef, OutboxStore};
    use crate::repository::RepositoryError;
    use crate::{
        CommitBatch, InMemoryOutboxStore, InMemoryRepository, OutboxMessage, OutboxMessageStatus,
        TransactionalCommit,
    };
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    struct RecordingPublisher {
        published: Mutex<Vec<String>>,
    }

    impl RecordingPublisher {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                published: Mutex::new(Vec::new()),
            })
        }
        fn ids(&self) -> Vec<String> {
            self.published.lock().unwrap().clone()
        }
    }

    impl MessagePublisher for Arc<RecordingPublisher> {
        async fn publish(&self, message: Message) -> Result<(), TransportError> {
            self.published
                .lock()
                .unwrap()
                .push(message.id().unwrap_or_default().to_string());
            Ok(())
        }
    }

    fn outbox(id: &str) -> OutboxMessage {
        OutboxMessage::create(id, "OrderCreated", b"{}".to_vec()).unwrap()
    }

    fn store_message(repo: &InMemoryRepository, message: OutboxMessage) -> String {
        let id = message.id().to_string();
        let mut batch = CommitBatch::empty();
        batch.outbox_messages.push(message);
        futures_executor_block(repo.commit_batch(batch)).unwrap();
        id
    }

    fn futures_executor_block<F: std::future::Future>(future: F) -> F::Output {
        // commit_batch is runtime-free; keep that path off the tokio worker.
        crate::outbox_worker::testing::block_on(future)
    }

    fn load(repo: &InMemoryRepository, id: &str) -> OutboxMessage {
        repo.outbox_storage()
            .read()
            .unwrap()
            .get(id)
            .unwrap()
            .clone()
    }

    #[tokio::test]
    async fn immediate_publish_leaves_nothing_for_the_drainer() {
        let repo = InMemoryRepository::new();
        let id = store_message(&repo, outbox("evt-1"));
        let publisher = RecordingPublisher::new();
        let immediate = OutboxDispatcher::new(
            repo.outbox_store(),
            publisher.clone(),
            "immediate:test",
            Duration::from_secs(60),
            3,
        );
        let outcome = immediate
            .dispatch_ids(std::slice::from_ref(&id))
            .await
            .unwrap();
        assert_eq!(outcome.published, 1);
        assert_eq!(load(&repo, &id).status, OutboxMessageStatus::Published);

        let drain = OutboxDrainRunner::new(OutboxDispatcher::new(
            repo.outbox_store(),
            publisher.clone(),
            drain_worker_id(),
            Duration::from_secs(60),
            3,
        ))
        .with_batch_size(8)
        .with_poll_interval(Duration::from_millis(5));
        let handle = drain.spawn();
        tokio::time::sleep(Duration::from_millis(30)).await;
        handle.stop().await.unwrap();

        assert_eq!(publisher.ids(), vec!["evt-1".to_string()]);
        assert!(repo.outbox_store().pending(8).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn drain_publishes_rows_the_immediate_path_never_attempted() {
        let repo = InMemoryRepository::new();
        store_message(&repo, outbox("evt-crash"));
        let publisher = RecordingPublisher::new();
        let drain = OutboxDrainRunner::for_store(repo.outbox_store(), publisher.clone(), 3)
            .with_batch_size(8)
            .with_poll_interval(Duration::from_millis(5));
        let handle = drain.spawn();
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if publisher.ids() == ["evt-crash"] {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("drain should publish the unclaimed row");
        handle.stop().await.unwrap();
        assert_eq!(
            load(&repo, "evt-crash").status,
            OutboxMessageStatus::Published
        );
    }

    #[tokio::test]
    async fn store_error_is_retried_and_the_task_survives() {
        let repo = InMemoryRepository::new();
        store_message(&repo, outbox("evt-retry"));
        let publisher = RecordingPublisher::new();
        let store = FlakyClaimStore {
            inner: repo.outbox_store(),
            fail_remaining: AtomicU32::new(2),
            claims: AtomicU32::new(0),
        };
        let drain = OutboxDrainRunner::for_store(store, publisher.clone(), 3)
            .with_batch_size(8)
            .with_poll_interval(Duration::from_millis(5))
            .with_error_backoff(Duration::from_millis(5))
            .with_max_error_backoff(Duration::from_millis(20));
        let handle = drain.spawn();
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if publisher.ids() == ["evt-retry"] {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("drain should recover after store errors");
        handle.stop().await.unwrap();
    }

    #[tokio::test]
    async fn stop_returns_promptly() {
        let repo = InMemoryRepository::new();
        let publisher = RecordingPublisher::new();
        let drain = OutboxDrainRunner::for_store(repo.outbox_store(), publisher, 3)
            .with_poll_interval(Duration::from_secs(30));
        let handle = drain.spawn();
        let started = std::time::Instant::now();
        handle.stop().await.unwrap();
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "stop should abort the idle sleep"
        );
    }

    struct FlakyClaimStore {
        inner: InMemoryOutboxStore,
        fail_remaining: AtomicU32,
        claims: AtomicU32,
    }

    impl OutboxStore for FlakyClaimStore {
        fn messages_by_status(
            &self,
            status: OutboxMessageStatus,
            limit: usize,
        ) -> impl Future<Output = Result<Vec<OutboxMessage>, RepositoryError>> + Send + '_ {
            self.inner.messages_by_status(status, limit)
        }

        fn claim<'a>(
            &'a self,
            request: ClaimOutboxMessages,
        ) -> impl Future<Output = Result<Vec<OutboxMessage>, RepositoryError>> + Send + 'a {
            async move {
                if self.fail_remaining.load(Ordering::SeqCst) > 0 {
                    self.fail_remaining.fetch_sub(1, Ordering::SeqCst);
                    return Err(RepositoryError::Storage {
                        operation: "injected claim failure".into(),
                        retryable: true,
                        source: None,
                    });
                }
                self.claims.fetch_add(1, Ordering::SeqCst);
                self.inner.claim(request).await
            }
        }

        fn complete<'a>(
            &'a self,
            claim: &'a OutboxClaimRef,
        ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
            self.inner.complete(claim)
        }

        fn release<'a>(
            &'a self,
            claim: &'a OutboxClaimRef,
            error: &'a str,
        ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
            self.inner.release(claim, error)
        }

        fn fail<'a>(
            &'a self,
            claim: &'a OutboxClaimRef,
            error: &'a str,
        ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
            self.inner.fail(claim, error)
        }
    }
}
