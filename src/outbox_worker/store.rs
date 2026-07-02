#![expect(
    clippy::manual_async_fn,
    reason = "async trait impls return impl Future + Send to preserve public Send bounds"
)]

use std::future::Future;
use std::time::{Duration, SystemTime};

use crate::hashmap_repo::HashMapOutboxStore;
use crate::outbox::{OutboxMessage, OutboxMessageStatus};
use crate::repository::RepositoryError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboxPublishFailureAction {
    Released,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimOutboxMessages {
    pub worker_id: String,
    pub batch_size: usize,
    pub lease: Duration,
    pub destination: Option<String>,
    /// Restrict the claim to this explicit set of message ids. `None` claims the
    /// next claimable batch in created-at order (normal worker polling); `Some`
    /// claims only the listed ids (after-commit immediate dispatch). Ids that are
    /// not currently claimable are simply skipped — a raced id is not an error.
    pub message_ids: Option<Vec<String>>,
}

impl ClaimOutboxMessages {
    pub fn new(worker_id: impl Into<String>, batch_size: usize, lease: Duration) -> Self {
        Self {
            worker_id: worker_id.into(),
            batch_size,
            lease,
            destination: None,
            message_ids: None,
        }
    }

    pub fn to_destination(mut self, destination: impl Into<String>) -> Self {
        self.destination = Some(destination.into());
        self
    }

    /// Restrict this claim to an explicit list of message ids (after-commit
    /// immediate dispatch). The batch size is bounded by the id count.
    pub fn for_ids(worker_id: impl Into<String>, ids: Vec<String>, lease: Duration) -> Self {
        Self {
            worker_id: worker_id.into(),
            batch_size: ids.len(),
            lease,
            destination: None,
            message_ids: Some(ids),
        }
    }

    /// Whether `id` is selectable under this request's id filter.
    fn selects(&self, id: &str) -> bool {
        match &self.message_ids {
            Some(ids) => ids.iter().any(|wanted| wanted == id),
            None => true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutboxClaimRef {
    pub message_id: String,
    pub worker_id: String,
    pub leased_until: SystemTime,
    pub attempt: u32,
}

impl OutboxClaimRef {
    pub fn from_message(message: &OutboxMessage) -> Result<Self, RepositoryError> {
        let worker_id = message
            .worker_id
            .clone()
            .ok_or_else(|| invalid_outbox_state(message, "outbox claim worker"))?;
        let leased_until = message
            .leased_until
            .ok_or_else(|| invalid_outbox_state(message, "outbox claim lease"))?;

        Ok(Self {
            message_id: message.id().to_string(),
            worker_id,
            leased_until,
            attempt: message.attempts,
        })
    }
}

/// Store capability for claiming and updating durable outbox messages.
pub trait OutboxStore: Send + Sync {
    fn messages_by_status(
        &self,
        status: OutboxMessageStatus,
    ) -> impl Future<Output = Result<Vec<OutboxMessage>, RepositoryError>> + Send + '_;

    fn pending(
        &self,
    ) -> impl Future<Output = Result<Vec<OutboxMessage>, RepositoryError>> + Send + '_ {
        async move { self.messages_by_status(OutboxMessageStatus::Pending).await }
    }

    fn claim<'a>(
        &'a self,
        request: ClaimOutboxMessages,
    ) -> impl Future<Output = Result<Vec<OutboxMessage>, RepositoryError>> + Send + 'a;

    fn complete<'a>(
        &'a self,
        claim: &'a OutboxClaimRef,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a;

    /// Complete a batch of claims after their rows were published.
    ///
    /// Equivalent to calling [`complete`] for each claim; backends override it
    /// to settle the batch in fewer round trips (one statement/transaction).
    /// Error semantics match the serial loop: a claim that is no longer
    /// completable (missing row, stale worker/attempt, expired lease) surfaces
    /// the same `NotFound`/`InvalidState` error, and other claims in the batch
    /// may already have been completed when the error is returned.
    ///
    /// [`complete`]: OutboxStore::complete
    fn complete_many<'a>(
        &'a self,
        claims: &'a [OutboxClaimRef],
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        async move {
            for claim in claims {
                self.complete(claim).await?;
            }
            Ok(())
        }
    }

    fn release<'a>(
        &'a self,
        claim: &'a OutboxClaimRef,
        error: &'a str,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a;

    fn fail<'a>(
        &'a self,
        claim: &'a OutboxClaimRef,
        error: &'a str,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a;

    fn record_failure<'a>(
        &'a self,
        claim: &'a OutboxClaimRef,
        error: &'a str,
        max_attempts: u32,
    ) -> impl Future<Output = Result<OutboxPublishFailureAction, RepositoryError>> + Send + 'a {
        async move {
            if claim.attempt >= max_attempts {
                self.fail(claim, error).await?;
                Ok(OutboxPublishFailureAction::Failed)
            } else {
                self.release(claim, error).await?;
                Ok(OutboxPublishFailureAction::Released)
            }
        }
    }
}

fn outbox_state(message: &OutboxMessage) -> String {
    format!(
        "{:?}, worker={:?}, leased_until={:?}, attempts={}",
        message.status, message.worker_id, message.leased_until, message.attempts
    )
}

fn invalid_outbox_state(message: &OutboxMessage, expected: &'static str) -> RepositoryError {
    RepositoryError::InvalidState {
        id: message.id().to_string(),
        expected,
        actual: outbox_state(message),
    }
}

pub(crate) fn ensure_active_claim(
    message: &OutboxMessage,
    claim: Option<&OutboxClaimRef>,
    now: SystemTime,
) -> Result<(), RepositoryError> {
    if !message.is_in_flight() {
        return Err(invalid_outbox_state(message, "in-flight outbox message"));
    }

    if let Some(claim) = claim {
        if !message.is_claimed_by(&claim.worker_id) {
            return Err(invalid_outbox_state(
                message,
                "outbox claim held by requesting worker",
            ));
        }

        if message.attempts != claim.attempt {
            return Err(invalid_outbox_state(
                message,
                "outbox claim attempt held by requesting worker",
            ));
        }
    }

    if message.has_expired_lease_at(now) {
        return Err(invalid_outbox_state(message, "unexpired outbox claim"));
    }

    Ok(())
}

fn sort_by_claim_order(messages: &mut [OutboxMessage]) {
    messages.sort_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then_with(|| left.id().cmp(right.id()))
    });
}

fn claim_order_ids<'a>(messages: impl Iterator<Item = &'a OutboxMessage>) -> Vec<String> {
    let mut entries = messages
        .map(|message| (message.created_at, message.id().to_string()))
        .collect::<Vec<_>>();
    entries.sort_by(|(left_time, left_id), (right_time, right_id)| {
        left_time
            .cmp(right_time)
            .then_with(|| left_id.cmp(right_id))
    });
    entries.into_iter().map(|(_, id)| id).collect()
}

impl HashMapOutboxStore {
    fn update_outbox_message<T>(
        &self,
        message_id: &str,
        update: impl FnOnce(&mut OutboxMessage) -> Result<T, RepositoryError>,
    ) -> Result<T, RepositoryError> {
        let mut storage = self
            .storage
            .write()
            .map_err(|_| RepositoryError::LockPoisoned("outbox write"))?;

        let message = storage
            .get_mut(message_id)
            .ok_or_else(|| RepositoryError::NotFound {
                id: message_id.to_string(),
            })?;

        update(message)
    }
}

impl OutboxStore for HashMapOutboxStore {
    fn messages_by_status(
        &self,
        status: OutboxMessageStatus,
    ) -> impl Future<Output = Result<Vec<OutboxMessage>, RepositoryError>> + Send + '_ {
        async move {
            let storage = self
                .storage
                .read()
                .map_err(|_| RepositoryError::LockPoisoned("outbox read"))?;

            let mut messages = storage
                .values()
                .filter(|message| message.status == status)
                .cloned()
                .collect::<Vec<_>>();
            sort_by_claim_order(&mut messages);
            Ok(messages)
        }
    }

    fn claim<'a>(
        &'a self,
        request: ClaimOutboxMessages,
    ) -> impl Future<Output = Result<Vec<OutboxMessage>, RepositoryError>> + Send + 'a {
        async move {
            let mut storage = self
                .storage
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("outbox write"))?;

            if request.batch_size == 0 {
                return Ok(Vec::new());
            }

            let now = SystemTime::now();
            let ids = claim_order_ids(storage.values());
            let mut claimed = Vec::new();
            for id in ids {
                if !request.selects(&id) {
                    continue;
                }

                let Some(message) = storage.get_mut(&id) else {
                    continue;
                };

                if message.is_claimable_at(now) {
                    if let Some(destination) = request.destination.as_deref() {
                        if message.destination.as_deref() != Some(destination) {
                            continue;
                        }
                    }
                    message.claim_at(&request.worker_id, request.lease, now)?;
                    claimed.push(message.clone());
                }

                if claimed.len() >= request.batch_size {
                    break;
                }
            }

            Ok(claimed)
        }
    }

    fn complete<'a>(
        &'a self,
        claim: &'a OutboxClaimRef,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        async move {
            self.update_outbox_message(&claim.message_id, |message| {
                ensure_active_claim(message, Some(claim), SystemTime::now())?;
                message.complete()?;
                Ok(())
            })
        }
    }

    /// Batched complete under a single write lock instead of one lock
    /// acquisition per claim. Same per-claim validation as [`complete`];
    /// claims before a failing one stay completed, like the serial loop.
    ///
    /// [`complete`]: OutboxStore::complete
    fn complete_many<'a>(
        &'a self,
        claims: &'a [OutboxClaimRef],
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        async move {
            if claims.is_empty() {
                return Ok(());
            }
            let mut storage = self
                .storage
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("outbox write"))?;
            let now = SystemTime::now();
            for claim in claims {
                let message = storage.get_mut(&claim.message_id).ok_or_else(|| {
                    RepositoryError::NotFound {
                        id: claim.message_id.clone(),
                    }
                })?;
                ensure_active_claim(message, Some(claim), now)?;
                message.complete()?;
            }
            Ok(())
        }
    }

    fn release<'a>(
        &'a self,
        claim: &'a OutboxClaimRef,
        error: &'a str,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        async move {
            self.update_outbox_message(&claim.message_id, |message| {
                ensure_active_claim(message, Some(claim), SystemTime::now())?;
                message.release(error.to_string())?;
                Ok(())
            })
        }
    }

    fn fail<'a>(
        &'a self,
        claim: &'a OutboxClaimRef,
        error: &'a str,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        async move {
            self.update_outbox_message(&claim.message_id, |message| {
                ensure_active_claim(message, Some(claim), SystemTime::now())?;
                message.fail(error.to_string())?;
                Ok(())
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CommitBatch, HashMapRepository, TransactionalCommit};
    use std::sync::{Arc, Barrier};
    use std::thread;

    async fn store_message(repo: &HashMapRepository, message: OutboxMessage) -> String {
        let id = message.id().to_string();
        let mut batch = CommitBatch::empty();
        batch.outbox_messages.push(message);
        repo.commit_batch(batch).await.unwrap();
        id
    }

    fn load_message(repo: &HashMapRepository, id: &str) -> OutboxMessage {
        repo.outbox_storage()
            .read()
            .unwrap()
            .get(id)
            .unwrap()
            .clone()
    }

    #[tokio::test]
    async fn claim_includes_expired_in_flight_messages() {
        let repo = HashMapRepository::new();
        let mut message = OutboxMessage::create("msg-1", "Event", b"{}".to_vec()).unwrap();
        message
            .claim_at("worker-1", Duration::from_secs(1), SystemTime::UNIX_EPOCH)
            .unwrap();
        let id = store_message(&repo, message).await;

        let store = repo.outbox_store();
        let claimed = store
            .claim(ClaimOutboxMessages::new(
                "worker-2",
                1,
                Duration::from_secs(60),
            ))
            .await
            .unwrap();

        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].worker_id.as_deref(), Some("worker-2"));
        assert_eq!(claimed[0].attempts, 2);

        let stored = load_message(&repo, &id);
        assert_eq!(stored.worker_id.as_deref(), Some("worker-2"));
        assert_eq!(stored.attempts, 2);
        assert!(stored.is_in_flight());
    }

    #[tokio::test]
    async fn claim_skips_unexpired_in_flight_messages() {
        let repo = HashMapRepository::new();
        let mut message = OutboxMessage::create("msg-1", "Event", b"{}".to_vec()).unwrap();
        message
            .claim_for("worker-1", Duration::from_secs(60))
            .unwrap();
        let id = store_message(&repo, message).await;

        let store = repo.outbox_store();
        let claimed = store
            .claim(ClaimOutboxMessages::new(
                "worker-2",
                1,
                Duration::from_secs(60),
            ))
            .await
            .unwrap();

        assert!(claimed.is_empty());
        let stored = load_message(&repo, &id);
        assert_eq!(stored.worker_id.as_deref(), Some("worker-1"));
        assert_eq!(stored.attempts, 1);
    }

    #[tokio::test]
    async fn claim_uses_created_at_before_message_id_order() {
        let repo = HashMapRepository::new();
        let mut newer = OutboxMessage::create("msg-a", "Event", b"{}".to_vec()).unwrap();
        newer.created_at = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
        let mut older = OutboxMessage::create("msg-z", "Event", b"{}".to_vec()).unwrap();
        older.created_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1);
        store_message(&repo, newer).await;
        store_message(&repo, older).await;

        let claimed = repo
            .outbox_store()
            .claim(ClaimOutboxMessages::new(
                "worker-1",
                1,
                Duration::from_secs(60),
            ))
            .await
            .unwrap();

        assert_eq!(claimed[0].id(), "msg-z");
    }

    #[tokio::test]
    async fn claim_by_explicit_ids_claims_only_requested() {
        let repo = HashMapRepository::new();
        store_message(
            &repo,
            OutboxMessage::create("msg-a", "Event", b"{}".to_vec()).unwrap(),
        )
        .await;
        store_message(
            &repo,
            OutboxMessage::create("msg-b", "Event", b"{}".to_vec()).unwrap(),
        )
        .await;
        store_message(
            &repo,
            OutboxMessage::create("msg-c", "Event", b"{}".to_vec()).unwrap(),
        )
        .await;

        let claimed = repo
            .outbox_store()
            .claim(ClaimOutboxMessages::for_ids(
                "worker-1",
                vec!["msg-b".to_string(), "msg-c".to_string()],
                Duration::from_secs(60),
            ))
            .await
            .unwrap();

        let mut claimed_ids = claimed
            .iter()
            .map(|m| m.id().to_string())
            .collect::<Vec<_>>();
        claimed_ids.sort();
        assert_eq!(claimed_ids, vec!["msg-b".to_string(), "msg-c".to_string()]);
        // The unrequested row stays pending.
        assert!(load_message(&repo, "msg-a").is_pending());
    }

    #[tokio::test]
    async fn claim_by_ids_skips_unclaimable_without_error() {
        let repo = HashMapRepository::new();
        let mut leased = OutboxMessage::create("msg-a", "Event", b"{}".to_vec()).unwrap();
        leased
            .claim_for("other-worker", Duration::from_secs(60))
            .unwrap();
        store_message(&repo, leased).await;

        // Requesting a currently-leased id (and a missing id) yields no claim,
        // not an error.
        let claimed = repo
            .outbox_store()
            .claim(ClaimOutboxMessages::for_ids(
                "worker-1",
                vec!["msg-a".to_string(), "missing".to_string()],
                Duration::from_secs(60),
            ))
            .await
            .unwrap();

        assert!(claimed.is_empty());
    }

    #[test]
    fn sort_by_claim_order_uses_message_id_tiebreaker() {
        let mut later = OutboxMessage::create("msg-c", "Event", b"{}".to_vec()).unwrap();
        later.created_at = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
        let mut second = OutboxMessage::create("msg-b", "Event", b"{}".to_vec()).unwrap();
        second.created_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1);
        let mut first = OutboxMessage::create("msg-a", "Event", b"{}".to_vec()).unwrap();
        first.created_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1);
        let mut messages = vec![later, second, first];

        sort_by_claim_order(&mut messages);

        assert_eq!(
            messages
                .iter()
                .map(|message| message.id())
                .collect::<Vec<_>>(),
            vec!["msg-a", "msg-b", "msg-c"]
        );
    }

    #[tokio::test]
    async fn competing_workers_only_claim_message_once() {
        let repo = HashMapRepository::new();
        let message = OutboxMessage::create("msg-1", "Event", b"{}".to_vec()).unwrap();
        let id = store_message(&repo, message).await;

        let barrier = Arc::new(Barrier::new(3));
        let store_a = repo.outbox_store();
        let store_b = repo.outbox_store();
        let barrier_a = Arc::clone(&barrier);
        let barrier_b = Arc::clone(&barrier);

        let worker_a = thread::spawn(move || {
            barrier_a.wait();
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(store_a.claim(ClaimOutboxMessages::new(
                "worker-a",
                1,
                Duration::from_secs(60),
            )))
            .unwrap()
            .len()
        });
        let worker_b = thread::spawn(move || {
            barrier_b.wait();
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(store_b.claim(ClaimOutboxMessages::new(
                "worker-b",
                1,
                Duration::from_secs(60),
            )))
            .unwrap()
            .len()
        });

        barrier.wait();
        let total_claimed = worker_a.join().unwrap() + worker_b.join().unwrap();

        assert_eq!(total_claimed, 1);
        let stored = load_message(&repo, &id);
        assert!(stored.is_in_flight());
        assert_eq!(stored.attempts, 1);
    }

    #[tokio::test]
    async fn publish_failure_releases_until_retry_ceiling_then_fails() {
        let repo = HashMapRepository::new();
        let message = OutboxMessage::create("msg-1", "Event", b"{}".to_vec()).unwrap();
        let id = store_message(&repo, message).await;

        let store = repo.outbox_store();
        let claimed = store
            .claim(ClaimOutboxMessages::new(
                "worker-1",
                1,
                Duration::from_secs(60),
            ))
            .await
            .unwrap();
        let claim = OutboxClaimRef::from_message(&claimed[0]).unwrap();
        let action = store
            .record_failure(&claim, "first failure", 2)
            .await
            .unwrap();
        assert_eq!(action, OutboxPublishFailureAction::Released);

        let stored = load_message(&repo, &id);
        assert!(stored.is_pending());
        assert_eq!(stored.attempts, 1);
        assert_eq!(stored.last_error.as_deref(), Some("first failure"));

        let claimed = store
            .claim(ClaimOutboxMessages::new(
                "worker-1",
                1,
                Duration::from_secs(60),
            ))
            .await
            .unwrap();
        let claim = OutboxClaimRef::from_message(&claimed[0]).unwrap();
        let action = store
            .record_failure(&claim, "second failure", 2)
            .await
            .unwrap();
        assert_eq!(action, OutboxPublishFailureAction::Failed);

        let stored = load_message(&repo, &id);
        assert!(stored.is_failed());
        assert_eq!(stored.attempts, 2);
        assert_eq!(stored.last_error.as_deref(), Some("second failure"));
    }

    #[tokio::test]
    async fn missing_message_updates_return_not_found() {
        let store = HashMapOutboxStore {
            storage: Default::default(),
        };
        let claim = OutboxClaimRef {
            message_id: "missing".into(),
            worker_id: "worker-1".into(),
            leased_until: SystemTime::now(),
            attempt: 1,
        };

        let is_missing = |err: RepositoryError| matches!(&err, RepositoryError::NotFound { id } if id == "missing");
        assert!(is_missing(store.complete(&claim).await.unwrap_err()));
        assert!(is_missing(
            store.release(&claim, "error").await.unwrap_err()
        ));
        assert!(is_missing(store.fail(&claim, "error").await.unwrap_err()));
    }

    #[tokio::test]
    async fn stale_or_mismatched_claims_cannot_be_completed() {
        let repo = HashMapRepository::new();
        let message = OutboxMessage::create("msg-1", "Event", b"{}".to_vec()).unwrap();
        let _id = store_message(&repo, message).await;

        let store = repo.outbox_store();
        let claimed = store
            .claim(ClaimOutboxMessages::new(
                "worker-1",
                1,
                Duration::from_secs(60),
            ))
            .await
            .unwrap();
        let mut claim = OutboxClaimRef::from_message(&claimed[0]).unwrap();
        claim.worker_id = "worker-2".into();
        let err = store.complete(&claim).await.unwrap_err();
        assert!(matches!(err, RepositoryError::InvalidState { .. }));

        let mut expired = OutboxMessage::create("msg-2", "Event", b"{}".to_vec()).unwrap();
        expired
            .claim_at("worker-1", Duration::from_secs(1), SystemTime::UNIX_EPOCH)
            .unwrap();
        let expired_id = store_message(&repo, expired).await;
        let expired = load_message(&repo, &expired_id);
        let claim = OutboxClaimRef::from_message(&expired).unwrap();
        let err = store.complete(&claim).await.unwrap_err();
        assert!(matches!(err, RepositoryError::InvalidState { .. }));
    }

    #[tokio::test]
    async fn stale_attempt_claims_cannot_complete_later_claims() {
        let repo = HashMapRepository::new();
        let message = OutboxMessage::create("msg-1", "Event", b"{}".to_vec()).unwrap();
        let _id = store_message(&repo, message).await;

        let store = repo.outbox_store();
        let claimed = store
            .claim(ClaimOutboxMessages::new(
                "worker-1",
                1,
                Duration::from_secs(60),
            ))
            .await
            .unwrap();
        let stale_claim = OutboxClaimRef::from_message(&claimed[0]).unwrap();
        store.release(&stale_claim, "retry").await.unwrap();

        let claimed = store
            .claim(ClaimOutboxMessages::new(
                "worker-1",
                1,
                Duration::from_secs(60),
            ))
            .await
            .unwrap();
        let current_claim = OutboxClaimRef::from_message(&claimed[0]).unwrap();

        let err = store.complete(&stale_claim).await.unwrap_err();
        assert!(matches!(err, RepositoryError::InvalidState { .. }));
        store.complete(&current_claim).await.unwrap();
    }

    #[tokio::test]
    async fn complete_many_completes_the_whole_batch() {
        let repo = HashMapRepository::new();
        for id in ["msg-1", "msg-2", "msg-3"] {
            store_message(
                &repo,
                OutboxMessage::create(id, "Event", b"{}".to_vec()).unwrap(),
            )
            .await;
        }

        let store = repo.outbox_store();
        let claimed = store
            .claim(ClaimOutboxMessages::new(
                "worker-1",
                3,
                Duration::from_secs(60),
            ))
            .await
            .unwrap();
        let claims = claimed
            .iter()
            .map(OutboxClaimRef::from_message)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        store.complete_many(&claims).await.unwrap();

        for id in ["msg-1", "msg-2", "msg-3"] {
            assert!(load_message(&repo, id).is_published());
        }
    }

    #[tokio::test]
    async fn complete_many_rejects_stale_and_missing_claims() {
        let repo = HashMapRepository::new();
        store_message(
            &repo,
            OutboxMessage::create("msg-1", "Event", b"{}".to_vec()).unwrap(),
        )
        .await;

        let store = repo.outbox_store();
        let claimed = store
            .claim(ClaimOutboxMessages::new(
                "worker-1",
                1,
                Duration::from_secs(60),
            ))
            .await
            .unwrap();
        let claims = vec![OutboxClaimRef::from_message(&claimed[0]).unwrap()];

        store.complete_many(&claims).await.unwrap();
        // Re-settling the now-published row is a stale claim, same as `complete`.
        let err = store.complete_many(&claims).await.unwrap_err();
        assert!(matches!(err, RepositoryError::InvalidState { .. }));

        let missing = vec![OutboxClaimRef {
            message_id: "missing".into(),
            worker_id: "worker-1".into(),
            leased_until: SystemTime::now(),
            attempt: 1,
        }];
        let err = store.complete_many(&missing).await.unwrap_err();
        assert!(matches!(err, RepositoryError::NotFound { id } if id == "missing"));
    }

    #[tokio::test]
    async fn already_published_message_is_not_completed_again() {
        let repo = HashMapRepository::new();
        let message = OutboxMessage::create("msg-1", "Event", b"{}".to_vec()).unwrap();
        let _id = store_message(&repo, message).await;

        let store = repo.outbox_store();
        let claimed = store
            .claim(ClaimOutboxMessages::new(
                "worker-1",
                1,
                Duration::from_secs(60),
            ))
            .await
            .unwrap();
        let claim = OutboxClaimRef::from_message(&claimed[0]).unwrap();
        store.complete(&claim).await.unwrap();

        let err = store.complete(&claim).await.unwrap_err();
        assert!(matches!(err, RepositoryError::InvalidState { .. }));
    }
}
