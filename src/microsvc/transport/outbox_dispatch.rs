//! Outbox → transport bridge.
//!
//! Maps durable [`OutboxMessage`] rows to the canonical [`Message`] and dispatches
//! them through an [`AsyncMessagePublisher`]. The same claim → map → publish →
//! settle path is shared by background worker polling ([`dispatch_batch`]) and
//! after-commit immediate dispatch ([`dispatch_ids`]), so the two cannot diverge
//! and cannot publish the same row concurrently — both go through the outbox
//! claim lease.
//!
//! [`dispatch_batch`]: OutboxDispatcher::dispatch_batch
//! [`dispatch_ids`]: OutboxDispatcher::dispatch_ids

use std::time::Duration;

use super::publisher::AsyncMessagePublisher;
use super::TransportError;
use crate::microsvc::{Message, MessageKind};
use crate::outbox::OutboxMessage;
use crate::outbox_worker::{
    AsyncOutboxStore, ClaimOutboxMessages, OutboxClaimRef, OutboxPublishFailureAction,
};

/// Content type for an outbox payload. Outbox payloads are codec-encoded bytes
/// (bitcode or raw), so the media type is binary; the exact codec travels in
/// metadata under [`SOURCED_METADATA_PREFIX`] for the consumer.
const OUTBOX_CONTENT_TYPE: &str = "application/octet-stream";

/// Reserved metadata key prefix for framework-derived keys. User metadata must
/// not use this prefix; keys here (payload codec, destination, source context)
/// carry decode/routing semantics and must not be shadowable by user metadata.
pub const SOURCED_METADATA_PREFIX: &str = "x-sourced-";

impl From<&OutboxMessage> for Message {
    /// Map a durable outbox row to a canonical transport message.
    ///
    /// - `id` ← outbox message id (the stable durable id);
    /// - `name` ← `event_type`;
    /// - `kind` ← `Command` when a point-to-point `destination` is set, else `Event`;
    /// - `payload` ← raw codec bytes, `content_type` = `application/octet-stream`;
    /// - `metadata` ← the outbox metadata (correlation/causation/trace/auth) plus
    ///   framework-derived keys under the reserved [`SOURCED_METADATA_PREFIX`]
    ///   namespace (payload codec, destination, source-aggregate context) so
    ///   decode/routing context can never be shadowed by a user metadata key.
    fn from(outbox: &OutboxMessage) -> Self {
        let kind = if outbox.destination.is_some() {
            MessageKind::Command
        } else {
            MessageKind::Event
        };

        // User metadata first (correlation/trace/auth), then framework-derived
        // keys under the reserved `x-sourced-` prefix. Any user key in the
        // reserved namespace is dropped so framework values stay authoritative
        // and cannot be shadowed on case-insensitive lookup.
        let mut metadata: Vec<(String, String)> = outbox
            .metadata
            .iter()
            .filter(|(key, _)| {
                !key.to_ascii_lowercase()
                    .starts_with(SOURCED_METADATA_PREFIX)
            })
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        metadata.push((
            format!("{SOURCED_METADATA_PREFIX}payload-codec"),
            outbox.payload_codec.clone(),
        ));
        metadata.push((
            format!("{SOURCED_METADATA_PREFIX}payload-codec-version"),
            outbox.payload_codec_version.to_string(),
        ));
        if let Some(destination) = &outbox.destination {
            metadata.push((
                format!("{SOURCED_METADATA_PREFIX}destination"),
                destination.clone(),
            ));
        }
        if let Some(source_type) = &outbox.source_aggregate_type {
            metadata.push((
                format!("{SOURCED_METADATA_PREFIX}source-aggregate-type"),
                source_type.clone(),
            ));
        }
        if let Some(source_id) = &outbox.source_aggregate_id {
            metadata.push((
                format!("{SOURCED_METADATA_PREFIX}source-aggregate-id"),
                source_id.clone(),
            ));
        }
        if let Some(sequence) = outbox.source_sequence {
            metadata.push((
                format!("{SOURCED_METADATA_PREFIX}source-sequence"),
                sequence.to_string(),
            ));
        }

        Message {
            id: Some(outbox.id().to_string()),
            name: outbox.event_type.clone(),
            kind,
            payload: outbox.payload.clone(),
            content_type: OUTBOX_CONTENT_TYPE.to_string(),
            metadata,
        }
    }
}

/// Counts of what one dispatch pass did. Raced/unclaimable ids are reflected as
/// `claimed < requested`, not as an error; publish failures are `released`
/// (retryable) or `failed` (attempt ceiling reached), not errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OutboxDispatchOutcome {
    /// Rows asked for (id count for `dispatch_ids`, batch size for `dispatch_batch`).
    pub requested: usize,
    /// Rows actually claimed this pass.
    pub claimed: usize,
    /// Rows published and completed.
    pub published: usize,
    /// Rows released for retry after a publish failure.
    pub released: usize,
    /// Rows permanently failed after exhausting attempts.
    pub failed: usize,
}

/// Bridges outbox claims to an [`AsyncMessagePublisher`], shared by immediate
/// after-commit dispatch and background worker polling.
///
/// Publish failures are not errors — they are reflected in
/// [`OutboxDispatchOutcome`] (released/failed) and the pass continues. A *store*
/// error (claim/complete/record_failure) does abort the pass and is returned as
/// `Err`; the partial [`OutboxDispatchOutcome`] is not returned in that case, and
/// any already-settled rows keep their settled state (the next pass resumes the
/// rest). Rows still leased by the aborted pass become claimable again at lease
/// expiry.
pub struct OutboxDispatcher<S, P> {
    store: S,
    publisher: P,
    worker_id: String,
    lease: Duration,
    max_attempts: u32,
}

impl<S, P> OutboxDispatcher<S, P>
where
    S: AsyncOutboxStore,
    P: AsyncMessagePublisher,
{
    /// Create a dispatcher. `worker_id` scopes claims (use a synthetic id such as
    /// `immediate:<process>` for after-commit dispatch); `max_attempts` is the
    /// publish-failure ceiling before a row is permanently failed.
    pub fn new(
        store: S,
        publisher: P,
        worker_id: impl Into<String>,
        lease: Duration,
        max_attempts: u32,
    ) -> Self {
        Self {
            store,
            publisher,
            worker_id: worker_id.into(),
            lease,
            max_attempts,
        }
    }

    /// Immediate after-commit dispatch of the explicit outbox ids a commit just
    /// inserted. Claims those ids (raced/unclaimable ids are skipped, not an
    /// error) before publishing, so it never races the polling worker.
    pub async fn dispatch_ids(
        &self,
        ids: &[String],
    ) -> Result<OutboxDispatchOutcome, TransportError> {
        let request =
            ClaimOutboxMessages::for_ids(self.worker_id.clone(), ids.to_vec(), self.lease);
        let claimed = self.store.claim_async(request).await?;
        let mut outcome = self.dispatch_claimed(claimed).await?;
        outcome.requested = ids.len();
        Ok(outcome)
    }

    /// Background worker dispatch of the next claimable batch.
    pub async fn dispatch_batch(
        &self,
        batch_size: usize,
    ) -> Result<OutboxDispatchOutcome, TransportError> {
        let request = ClaimOutboxMessages::new(self.worker_id.clone(), batch_size, self.lease);
        let claimed = self.store.claim_async(request).await?;
        let mut outcome = self.dispatch_claimed(claimed).await?;
        outcome.requested = batch_size;
        Ok(outcome)
    }

    /// The shared settle path: map → publish → complete on success, or
    /// record_failure (release/fail) on publish error. A row is completed ONLY
    /// after the publish threshold; an unknown/failed publish leaves it retryable.
    async fn dispatch_claimed(
        &self,
        claimed: Vec<OutboxMessage>,
    ) -> Result<OutboxDispatchOutcome, TransportError> {
        let mut outcome = OutboxDispatchOutcome {
            claimed: claimed.len(),
            ..Default::default()
        };
        for message in claimed {
            let claim = OutboxClaimRef::from_message(&message)?;
            let transport_message = Message::from(&message);
            match self.publisher.publish(transport_message).await {
                Ok(()) => {
                    self.store.complete_async(&claim).await?;
                    outcome.published += 1;
                }
                Err(publish_error) => {
                    match self
                        .store
                        .record_failure_async(&claim, &publish_error.to_string(), self.max_attempts)
                        .await?
                    {
                        OutboxPublishFailureAction::Released => outcome.released += 1,
                        OutboxPublishFailureAction::Failed => outcome.failed += 1,
                    }
                }
            }
        }
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HashMapRepository, TransactionalCommit};
    use std::future::Future;
    use std::sync::Mutex;

    fn block_on<F: Future>(future: F) -> F::Output {
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

    /// Publisher that records every published message id, optionally failing.
    struct RecordingPublisher {
        published: Mutex<Vec<String>>,
        fail: bool,
    }

    impl RecordingPublisher {
        fn new(fail: bool) -> Self {
            Self {
                published: Mutex::new(Vec::new()),
                fail,
            }
        }
        fn ids(&self) -> Vec<String> {
            self.published.lock().unwrap().clone()
        }
    }

    impl AsyncMessagePublisher for RecordingPublisher {
        async fn publish(&self, message: Message) -> Result<(), TransportError> {
            if self.fail {
                // Unknown outcome: surface as a retryable error.
                return Err(TransportError::retryable("publish failed"));
            }
            self.published
                .lock()
                .unwrap()
                .push(message.id().unwrap_or_default().to_string());
            Ok(())
        }
    }

    fn store_message(repo: &HashMapRepository, message: OutboxMessage) -> String {
        let id = message.id().to_string();
        let mut batch = crate::CommitBatch::empty();
        batch.outbox_messages.push(message);
        repo.commit_batch(batch).unwrap();
        id
    }

    fn outbox(id: &str) -> OutboxMessage {
        OutboxMessage::create_with_metadata(
            id,
            "OrderCreated",
            b"\x01\x02".to_vec(),
            [("correlation_id".to_string(), "corr-1".to_string())]
                .into_iter()
                .collect(),
        )
        .unwrap()
    }

    #[test]
    fn maps_outbox_row_to_canonical_message() {
        let message = Message::from(&outbox("evt-1"));
        assert_eq!(message.id(), Some("evt-1"));
        assert_eq!(message.name(), "OrderCreated");
        assert_eq!(message.kind, MessageKind::Event);
        assert_eq!(message.payload(), b"\x01\x02");
        assert_eq!(message.content_type, "application/octet-stream");
        // User metadata preserved; codec carried (namespaced) for the consumer.
        assert_eq!(message.correlation_id(), Some("corr-1"));
        assert_eq!(message.metadata("x-sourced-payload-codec"), Some("bytes"));
        assert_eq!(
            message.metadata("x-sourced-payload-codec-version"),
            Some("1")
        );
    }

    #[test]
    fn user_metadata_cannot_shadow_reserved_framework_keys() {
        // A user metadata key colliding with a reserved name does not override
        // the framework value, because framework keys are namespaced.
        let outbox = OutboxMessage::create_with_metadata(
            "evt-1",
            "OrderCreated",
            b"\x01".to_vec(),
            [("x-sourced-payload-codec".to_string(), "evil".to_string())]
                .into_iter()
                .collect(),
        )
        .unwrap();
        let message = Message::from(&outbox);
        // The user's reserved-prefix key is dropped; lookup returns the
        // authoritative framework value, and there is exactly one such entry.
        assert_eq!(message.metadata("x-sourced-payload-codec"), Some("bytes"));
        assert_eq!(
            message
                .metadata
                .iter()
                .filter(|(k, _)| k == "x-sourced-payload-codec")
                .count(),
            1
        );
    }

    #[test]
    fn destination_maps_to_command_kind() {
        let outbox =
            OutboxMessage::create_to("cmd-1", "ShipOrder", "shipping", b"{}".to_vec()).unwrap();
        let message = Message::from(&outbox);
        assert_eq!(message.kind, MessageKind::Command);
        assert_eq!(message.metadata("x-sourced-destination"), Some("shipping"));
    }

    fn dispatcher(
        repo: &HashMapRepository,
        fail: bool,
        max_attempts: u32,
    ) -> OutboxDispatcher<crate::HashMapOutboxStore, RecordingPublisher> {
        OutboxDispatcher::new(
            repo.outbox_store(),
            RecordingPublisher::new(fail),
            "immediate:test",
            Duration::from_secs(60),
            max_attempts,
        )
    }

    fn load(repo: &HashMapRepository, id: &str) -> OutboxMessage {
        repo.outbox_storage()
            .read()
            .unwrap()
            .get(id)
            .unwrap()
            .clone()
    }

    #[test]
    fn dispatch_ids_claims_then_publishes_then_completes() {
        let repo = HashMapRepository::new();
        let id = store_message(&repo, outbox("evt-1"));
        let dispatcher = dispatcher(&repo, false, 3);

        let outcome = block_on(dispatcher.dispatch_ids(std::slice::from_ref(&id))).unwrap();

        assert_eq!(
            outcome,
            OutboxDispatchOutcome {
                requested: 1,
                claimed: 1,
                published: 1,
                released: 0,
                failed: 0,
            }
        );
        // The publisher saw the message id, and the row is completed only after.
        assert_eq!(dispatcher.publisher.ids(), vec!["evt-1".to_string()]);
        assert!(load(&repo, &id).is_published());
    }

    #[test]
    fn unknown_publish_outcome_leaves_row_retryable() {
        let repo = HashMapRepository::new();
        let id = store_message(&repo, outbox("evt-1"));
        let dispatcher = dispatcher(&repo, true, 3);

        let outcome = block_on(dispatcher.dispatch_ids(std::slice::from_ref(&id))).unwrap();

        assert_eq!(outcome.published, 0);
        assert_eq!(outcome.released, 1);
        assert_eq!(outcome.failed, 0);
        // Released back to pending, not completed: still retryable.
        let row = load(&repo, &id);
        assert!(row.is_pending());
        assert_eq!(row.attempts, 1);
    }

    #[test]
    fn publish_failure_fails_row_at_attempt_ceiling() {
        let repo = HashMapRepository::new();
        let id = store_message(&repo, outbox("evt-1"));
        let dispatcher = dispatcher(&repo, true, 1);

        let outcome = block_on(dispatcher.dispatch_ids(std::slice::from_ref(&id))).unwrap();

        assert_eq!(outcome.failed, 1);
        assert_eq!(outcome.released, 0);
        assert!(load(&repo, &id).is_failed());
    }

    #[test]
    fn dispatch_ids_only_claims_requested_ids() {
        let repo = HashMapRepository::new();
        let wanted = store_message(&repo, outbox("evt-1"));
        let other = store_message(&repo, outbox("evt-2"));
        let dispatcher = dispatcher(&repo, false, 3);

        let outcome = block_on(dispatcher.dispatch_ids(std::slice::from_ref(&wanted))).unwrap();

        assert_eq!(outcome.claimed, 1);
        assert_eq!(outcome.published, 1);
        assert!(load(&repo, &wanted).is_published());
        // The unrequested row is untouched.
        assert!(load(&repo, &other).is_pending());
    }

    #[test]
    fn raced_id_is_not_an_error() {
        let repo = HashMapRepository::new();
        // No such row stored: claim returns nothing, dispatch is a clean no-op.
        let outcome =
            block_on(dispatcher(&repo, false, 3).dispatch_ids(&["missing".to_string()])).unwrap();
        assert_eq!(
            outcome,
            OutboxDispatchOutcome {
                requested: 1,
                claimed: 0,
                published: 0,
                released: 0,
                failed: 0,
            }
        );
    }

    #[test]
    fn worker_and_immediate_dispatch_share_state_transitions() {
        let repo = HashMapRepository::new();
        let immediate_id = store_message(&repo, outbox("evt-1"));
        let _worker_id = store_message(&repo, outbox("evt-2"));

        // Immediate dispatch claims+completes evt-1; worker batch then drains the
        // rest. Both go through the same claim/publish/complete path.
        let dispatcher = dispatcher(&repo, false, 3);
        let immediate =
            block_on(dispatcher.dispatch_ids(std::slice::from_ref(&immediate_id))).unwrap();
        assert_eq!(immediate.published, 1);

        let drained = block_on(dispatcher.dispatch_batch(10)).unwrap();
        assert_eq!(drained.claimed, 1, "only evt-2 remains claimable");
        assert_eq!(drained.published, 1);
        assert_eq!(dispatcher.publisher.ids().len(), 2);
    }
}
