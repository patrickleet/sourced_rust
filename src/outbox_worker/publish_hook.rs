//! [`OutboxPublishHook`] backed by an outbox store + a message publisher.
//!
//! This is what makes `repo.outbox(msg).commit(agg)` publish: `Service::with_bus`
//! installs one of these on the repository, and `OutboxCommit::commit` hands it
//! the rows it just committed-and-claimed. The hook publishes each row and
//! settles its claim — `complete` on success, `record_failure` (release/fail)
//! on a publish error so the row stays retryable for the polling worker — via
//! the same [`publish_and_settle`] path the dispatcher drains through. It never
//! re-claims: the rows were already claimed in the commit transaction.
//!
//! [`publish_and_settle`]: super::outbox_dispatch::publish_and_settle

use std::future::Future;
use std::pin::Pin;

use crate::bus::MessagePublisher;
use crate::outbox::{OutboxMessage, OutboxPublishHook};
use crate::repository::RepositoryError;

use super::outbox_dispatch::{publish_and_settle, SettleOutcome};
use super::OutboxStore;

/// Publishes committed outbox rows through `publisher` and settles their claims
/// in `store`. The `store` must be the same outbox store the commit wrote to.
pub struct BusOutboxPublishHook<S, P> {
    store: S,
    publisher: P,
    max_attempts: u32,
    service_name: Option<String>,
}

impl<S, P> BusOutboxPublishHook<S, P> {
    /// Build the hook from the outbox store, a message publisher (e.g. a
    /// `BusPublisher` over a `*Bus`), and the publish-failure ceiling.
    pub fn new(store: S, publisher: P, max_attempts: u32) -> Self {
        Self {
            store,
            publisher,
            max_attempts,
            service_name: None,
        }
    }

    /// Attach the logical service name to metrics emitted by this hook.
    pub fn with_service(mut self, service_name: Option<String>) -> Self {
        self.service_name = service_name;
        self
    }
}

impl<S, P> OutboxPublishHook for BusOutboxPublishHook<S, P>
where
    S: OutboxStore,
    P: MessagePublisher,
{
    fn publish_claimed<'a>(
        &'a self,
        claimed: Vec<OutboxMessage>,
    ) -> Pin<Box<dyn Future<Output = Result<(), RepositoryError>> + Send + 'a>> {
        Box::pin(async move {
            // Concurrency 1: a commit's rows are one aggregate's events, and
            // their relative order matters to consumers.
            let settled = publish_and_settle(
                &self.store,
                &self.publisher,
                claimed,
                self.max_attempts,
                std::num::NonZeroUsize::MIN,
                self.service_name.as_deref(),
            )
            .await?;
            self.record_outbox_outcomes(&settled).await;
            Ok(())
        })
    }
}

impl<S, P> BusOutboxPublishHook<S, P>
where
    S: OutboxStore,
{
    async fn record_outbox_outcomes(&self, settled: &SettleOutcome) {
        #[cfg(feature = "metrics")]
        {
            let service = self.service_name.as_deref();
            crate::metrics::record_outbox_messages(
                service,
                crate::telemetry::outbox_outcome::PUBLISHED,
                settled.published,
            );
            crate::metrics::record_outbox_messages(
                service,
                crate::telemetry::outbox_outcome::RELEASED,
                settled.released,
            );
            crate::metrics::record_outbox_messages(
                service,
                crate::telemetry::outbox_outcome::FAILED,
                settled.failed,
            );
            super::outbox_dispatch::record_backlog_gauges(&self.store, service).await;
        }
        #[cfg(not(feature = "metrics"))]
        let _ = settled;
    }
}

#[cfg(all(test, feature = "metrics"))]
mod tests {
    use std::time::{Duration, SystemTime};

    use crate::bus::{Message, TransportError};
    use crate::outbox_worker::{ClaimOutboxMessages, OutboxStore};
    use crate::{CommitBatch, InMemoryRepository, OutboxMessage, TransactionalCommit};

    use super::*;

    struct FailingPublisher;

    impl MessagePublisher for FailingPublisher {
        async fn publish(&self, _message: Message) -> Result<(), TransportError> {
            Err(TransportError::retryable("publish failed"))
        }
    }

    #[cfg(feature = "metrics")]
    #[tokio::test]
    async fn metrics_record_hook_publish_retry_age_and_backlog_gauges() {
        let _guard = crate::metrics::async_lock_for_tests().await;
        crate::metrics::reset_for_tests();

        let repo = InMemoryRepository::new();
        let store = repo.outbox_store();
        let mut message =
            OutboxMessage::create("hook-retry", "OrderCreated", b"{}".to_vec()).unwrap();
        message.created_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1);
        repo.commit_batch(CommitBatch {
            outbox_messages: vec![message],
            ..CommitBatch::empty()
        })
        .await
        .unwrap();

        let claimed = store
            .claim(ClaimOutboxMessages::new(
                "worker-1",
                1,
                Duration::from_secs(60),
            ))
            .await
            .unwrap();
        let claim = crate::outbox_worker::OutboxClaimRef::from_message(&claimed[0]).unwrap();
        store
            .record_failure(&claim, "first failure", 3)
            .await
            .unwrap();
        let retry_claimed = store
            .claim(ClaimOutboxMessages::new(
                "worker-1",
                1,
                Duration::from_secs(60),
            ))
            .await
            .unwrap();

        let hook = BusOutboxPublishHook::new(store, FailingPublisher, 3)
            .with_service(Some("orders-hook".to_string()));
        hook.publish_claimed(retry_claimed).await.unwrap();

        let text = crate::metrics::prometheus_text();
        assert!(
            text.contains(
                "distributed_outbox_messages_total{service=\"orders-hook\",outcome=\"released\"} 1"
            ),
            "hook metrics should include released outcome:\n{text}"
        );
        assert!(
            text.contains(
                "distributed_outbox_publish_duration_seconds_count{service=\"orders-hook\",message_kind=\"event\",outcome=\"released\"} 1"
            ),
            "hook metrics should include publish timing:\n{text}"
        );
        assert!(
            text.contains(
                "distributed_outbox_message_age_seconds_count{service=\"orders-hook\",phase=\"settled\",outcome=\"released\"} 1"
            ),
            "hook metrics should include message age at settlement:\n{text}"
        );
        assert!(
            text.contains(
                "distributed_outbox_retry_messages_total{service=\"orders-hook\",outcome=\"released\",attempt_bucket=\"2\"} 1"
            ),
            "hook metrics should include retry attempt bucket:\n{text}"
        );
        assert!(
            text.contains("distributed_outbox_pending_messages{service=\"orders-hook\"} 1"),
            "hook metrics should include pending backlog gauge:\n{text}"
        );
        assert!(
            text.contains("distributed_outbox_claimable_messages{service=\"orders-hook\"} 1"),
            "hook metrics should include claimable backlog gauge:\n{text}"
        );
    }
}
