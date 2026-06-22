//! [`OutboxPublishHook`] backed by an outbox store + a message publisher.
//!
//! This is what makes `repo.outbox(msg).commit(agg)` publish: `Service::with_bus`
//! installs one of these on the repository, and `OutboxCommit::commit` hands it
//! the row it just committed-and-claimed. The hook publishes the row and settles
//! its claim — `complete` on success, `record_failure` (release/fail) on a
//! publish error so the row stays retryable for the polling worker. It never
//! re-claims: the row was already claimed in the commit transaction.

use std::future::Future;
use std::pin::Pin;

use crate::bus::{Message, MessagePublisher};
use crate::outbox::{OutboxMessage, OutboxPublishHook};
use crate::repository::RepositoryError;

use super::{OutboxClaimRef, OutboxStore};

/// Publishes committed outbox rows through `publisher` and settles their claims
/// in `store`. The `store` must be the same outbox store the commit wrote to.
pub struct BusOutboxPublishHook<S, P> {
    store: S,
    publisher: P,
    max_attempts: u32,
}

impl<S, P> BusOutboxPublishHook<S, P> {
    /// Build the hook from the outbox store, a message publisher (e.g. a
    /// `BusPublisher` over a `*Bus`), and the publish-failure ceiling.
    pub fn new(store: S, publisher: P, max_attempts: u32) -> Self {
        Self {
            store,
            publisher,
            max_attempts,
        }
    }
}

impl<S, P> OutboxPublishHook for BusOutboxPublishHook<S, P>
where
    S: OutboxStore,
    P: MessagePublisher,
{
    fn publish_claimed<'a>(
        &'a self,
        claimed: OutboxMessage,
    ) -> Pin<Box<dyn Future<Output = Result<(), RepositoryError>> + Send + 'a>> {
        Box::pin(async move {
            let claim = OutboxClaimRef::from_message(&claimed)?;
            let message = Message::from(&claimed);
            match self.publisher.publish(message).await {
                Ok(()) => self.store.complete(&claim).await,
                Err(error) => self
                    .store
                    .record_failure(&claim, &error.to_string(), self.max_attempts)
                    .await
                    .map(|_action| ()),
            }
        })
    }
}
