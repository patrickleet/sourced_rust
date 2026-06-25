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
            crate::metrics::record_outbox_messages(service, "published", settled.published);
            crate::metrics::record_outbox_messages(service, "released", settled.released);
            crate::metrics::record_outbox_messages(service, "failed", settled.failed);
            super::outbox_dispatch::record_backlog_gauges(&self.store, service).await;
        }
        #[cfg(not(feature = "metrics"))]
        let _ = settled;
    }
}
