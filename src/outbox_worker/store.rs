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
}

impl ClaimOutboxMessages {
    pub fn new(worker_id: impl Into<String>, batch_size: usize, lease: Duration) -> Self {
        Self {
            worker_id: worker_id.into(),
            batch_size,
            lease,
            destination: None,
        }
    }

    pub fn to_destination(mut self, destination: impl Into<String>) -> Self {
        self.destination = Some(destination.into());
        self
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
    /// Return all outbox messages with the given status.
    fn messages_by_status(
        &self,
        status: OutboxMessageStatus,
    ) -> Result<Vec<OutboxMessage>, RepositoryError>;

    /// Return all pending outbox messages.
    fn pending(&self) -> Result<Vec<OutboxMessage>, RepositoryError> {
        self.messages_by_status(OutboxMessageStatus::Pending)
    }

    /// Claim pending outbox messages for processing.
    fn claim(&self, request: ClaimOutboxMessages) -> Result<Vec<OutboxMessage>, RepositoryError>;

    /// Mark an outbox message as completed if it is still claimed by this worker.
    fn complete(&self, claim: &OutboxClaimRef) -> Result<(), RepositoryError>;

    /// Release an outbox message if it is still claimed by this worker.
    fn release(&self, claim: &OutboxClaimRef, error: &str) -> Result<(), RepositoryError>;

    /// Mark an outbox message as permanently failed if it is still claimed by this worker.
    fn fail(&self, claim: &OutboxClaimRef, error: &str) -> Result<(), RepositoryError>;

    /// Record a publish failure for a claimed message, releasing it for retry
    /// or permanently failing it when the attempt ceiling is reached.
    fn record_failure(
        &self,
        claim: &OutboxClaimRef,
        error: &str,
        max_attempts: u32,
    ) -> Result<OutboxPublishFailureAction, RepositoryError>;
}

/// Async store capability for claiming and updating durable outbox messages.
pub trait AsyncOutboxStore: Send + Sync {
    fn messages_by_status_async(
        &self,
        status: OutboxMessageStatus,
    ) -> impl Future<Output = Result<Vec<OutboxMessage>, RepositoryError>> + Send + '_;

    fn pending_async(
        &self,
    ) -> impl Future<Output = Result<Vec<OutboxMessage>, RepositoryError>> + Send + '_ {
        async move {
            self.messages_by_status_async(OutboxMessageStatus::Pending)
                .await
        }
    }

    fn claim_async<'a>(
        &'a self,
        request: ClaimOutboxMessages,
    ) -> impl Future<Output = Result<Vec<OutboxMessage>, RepositoryError>> + Send + 'a;

    fn complete_async<'a>(
        &'a self,
        claim: &'a OutboxClaimRef,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a;

    fn release_async<'a>(
        &'a self,
        claim: &'a OutboxClaimRef,
        error: &'a str,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a;

    fn fail_async<'a>(
        &'a self,
        claim: &'a OutboxClaimRef,
        error: &'a str,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a;

    fn record_failure_async<'a>(
        &'a self,
        claim: &'a OutboxClaimRef,
        error: &'a str,
        max_attempts: u32,
    ) -> impl Future<Output = Result<OutboxPublishFailureAction, RepositoryError>> + Send + 'a;
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
    ) -> Result<Vec<OutboxMessage>, RepositoryError> {
        let storage = self
            .storage
            .read()
            .map_err(|_| RepositoryError::LockPoisoned("outbox read"))?;

        let mut messages = storage
            .values()
            .filter(|message| message.status == status)
            .cloned()
            .collect::<Vec<_>>();
        messages.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.id().cmp(right.id()))
        });
        Ok(messages)
    }

    fn claim(&self, request: ClaimOutboxMessages) -> Result<Vec<OutboxMessage>, RepositoryError> {
        let mut storage = self
            .storage
            .write()
            .map_err(|_| RepositoryError::LockPoisoned("outbox write"))?;

        if request.batch_size == 0 {
            return Ok(Vec::new());
        }

        let now = SystemTime::now();
        let mut ids = storage.keys().cloned().collect::<Vec<_>>();
        ids.sort();
        let mut claimed = Vec::new();
        for id in ids {
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

    fn complete(&self, claim: &OutboxClaimRef) -> Result<(), RepositoryError> {
        self.update_outbox_message(&claim.message_id, |message| {
            ensure_active_claim(message, Some(claim), SystemTime::now())?;
            message.complete()?;
            Ok(())
        })
    }

    fn release(&self, claim: &OutboxClaimRef, error: &str) -> Result<(), RepositoryError> {
        self.update_outbox_message(&claim.message_id, |message| {
            ensure_active_claim(message, Some(claim), SystemTime::now())?;
            message.release(error.to_string())?;
            Ok(())
        })
    }

    fn fail(&self, claim: &OutboxClaimRef, error: &str) -> Result<(), RepositoryError> {
        self.update_outbox_message(&claim.message_id, |message| {
            ensure_active_claim(message, Some(claim), SystemTime::now())?;
            message.fail(error.to_string())?;
            Ok(())
        })
    }

    fn record_failure(
        &self,
        claim: &OutboxClaimRef,
        error: &str,
        max_attempts: u32,
    ) -> Result<OutboxPublishFailureAction, RepositoryError> {
        self.update_outbox_message(&claim.message_id, |message| {
            ensure_active_claim(message, Some(claim), SystemTime::now())?;
            if message.attempts >= max_attempts {
                message.fail(error.to_string())?;
                Ok(OutboxPublishFailureAction::Failed)
            } else {
                message.release(error.to_string())?;
                Ok(OutboxPublishFailureAction::Released)
            }
        })
    }
}

impl AsyncOutboxStore for HashMapOutboxStore {
    fn messages_by_status_async(
        &self,
        status: OutboxMessageStatus,
    ) -> impl Future<Output = Result<Vec<OutboxMessage>, RepositoryError>> + Send + '_ {
        async move {
            let storage = self
                .storage
                .read()
                .map_err(|_| RepositoryError::LockPoisoned("async outbox read"))?;
            let mut messages = storage
                .values()
                .filter(|message| message.status == status)
                .cloned()
                .collect::<Vec<_>>();
            messages.sort_by_key(|message| message.created_at);
            Ok(messages)
        }
    }

    fn claim_async<'a>(
        &'a self,
        request: ClaimOutboxMessages,
    ) -> impl Future<Output = Result<Vec<OutboxMessage>, RepositoryError>> + Send + 'a {
        async move {
            if request.batch_size == 0 {
                return Ok(Vec::new());
            }

            let now = SystemTime::now();
            let mut storage = self
                .storage
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("async outbox write"))?;
            let mut ids = storage.keys().cloned().collect::<Vec<_>>();
            ids.sort();

            let mut claimed = Vec::new();
            for id in ids {
                let Some(message) = storage.get_mut(&id) else {
                    continue;
                };

                if !message.is_claimable_at(now) {
                    continue;
                }

                if let Some(destination) = request.destination.as_deref() {
                    if message.destination.as_deref() != Some(destination) {
                        continue;
                    }
                }

                message.claim_at(&request.worker_id, request.lease, now)?;
                claimed.push(message.clone());

                if claimed.len() >= request.batch_size {
                    break;
                }
            }

            Ok(claimed)
        }
    }

    fn complete_async<'a>(
        &'a self,
        claim: &'a OutboxClaimRef,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        async move {
            let mut storage = self
                .storage
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("async outbox write"))?;
            let message =
                storage
                    .get_mut(&claim.message_id)
                    .ok_or_else(|| RepositoryError::NotFound {
                        id: claim.message_id.clone(),
                    })?;
            ensure_active_claim(message, Some(claim), SystemTime::now())?;
            message.complete()?;
            Ok(())
        }
    }

    fn release_async<'a>(
        &'a self,
        claim: &'a OutboxClaimRef,
        error: &'a str,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        async move {
            let mut storage = self
                .storage
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("async outbox write"))?;
            let message =
                storage
                    .get_mut(&claim.message_id)
                    .ok_or_else(|| RepositoryError::NotFound {
                        id: claim.message_id.clone(),
                    })?;
            ensure_active_claim(message, Some(claim), SystemTime::now())?;
            message.release(error.to_string())?;
            Ok(())
        }
    }

    fn fail_async<'a>(
        &'a self,
        claim: &'a OutboxClaimRef,
        error: &'a str,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        async move {
            let mut storage = self
                .storage
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("async outbox write"))?;
            let message =
                storage
                    .get_mut(&claim.message_id)
                    .ok_or_else(|| RepositoryError::NotFound {
                        id: claim.message_id.clone(),
                    })?;
            ensure_active_claim(message, Some(claim), SystemTime::now())?;
            message.fail(error.to_string())?;
            Ok(())
        }
    }

    fn record_failure_async<'a>(
        &'a self,
        claim: &'a OutboxClaimRef,
        error: &'a str,
        max_attempts: u32,
    ) -> impl Future<Output = Result<OutboxPublishFailureAction, RepositoryError>> + Send + 'a {
        async move {
            let mut storage = self
                .storage
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("async outbox write"))?;
            let message =
                storage
                    .get_mut(&claim.message_id)
                    .ok_or_else(|| RepositoryError::NotFound {
                        id: claim.message_id.clone(),
                    })?;
            ensure_active_claim(message, Some(claim), SystemTime::now())?;
            if message.attempts >= max_attempts {
                message.fail(error.to_string())?;
                Ok(OutboxPublishFailureAction::Failed)
            } else {
                message.release(error.to_string())?;
                Ok(OutboxPublishFailureAction::Released)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HashMapRepository, TransactionalCommit};
    use std::sync::{Arc, Barrier};
    use std::thread;

    fn store_message(repo: &HashMapRepository, message: OutboxMessage) -> String {
        let id = message.id().to_string();
        let mut batch = crate::CommitBatch::empty();
        batch.outbox_messages.push(message);
        repo.commit_batch(batch).unwrap();
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

    #[test]
    fn claim_includes_expired_in_flight_messages() {
        let repo = HashMapRepository::new();
        let mut message = OutboxMessage::create("msg-1", "Event", b"{}".to_vec()).unwrap();
        message
            .claim_at("worker-1", Duration::from_secs(1), SystemTime::UNIX_EPOCH)
            .unwrap();
        let id = store_message(&repo, message);

        let store = repo.outbox_store();
        let claimed = store
            .claim(ClaimOutboxMessages::new(
                "worker-2",
                1,
                Duration::from_secs(60),
            ))
            .unwrap();

        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].worker_id.as_deref(), Some("worker-2"));
        assert_eq!(claimed[0].attempts, 2);

        let stored = load_message(&repo, &id);
        assert_eq!(stored.worker_id.as_deref(), Some("worker-2"));
        assert_eq!(stored.attempts, 2);
        assert!(stored.is_in_flight());
    }

    #[test]
    fn claim_skips_unexpired_in_flight_messages() {
        let repo = HashMapRepository::new();
        let mut message = OutboxMessage::create("msg-1", "Event", b"{}".to_vec()).unwrap();
        message
            .claim_for("worker-1", Duration::from_secs(60))
            .unwrap();
        let id = store_message(&repo, message);

        let store = repo.outbox_store();
        let claimed = store
            .claim(ClaimOutboxMessages::new(
                "worker-2",
                1,
                Duration::from_secs(60),
            ))
            .unwrap();

        assert!(claimed.is_empty());
        let stored = load_message(&repo, &id);
        assert_eq!(stored.worker_id.as_deref(), Some("worker-1"));
        assert_eq!(stored.attempts, 1);
    }

    #[test]
    fn competing_workers_only_claim_message_once() {
        let repo = HashMapRepository::new();
        let message = OutboxMessage::create("msg-1", "Event", b"{}".to_vec()).unwrap();
        let id = store_message(&repo, message);

        let barrier = Arc::new(Barrier::new(3));
        let store_a = repo.outbox_store();
        let store_b = repo.outbox_store();
        let barrier_a = Arc::clone(&barrier);
        let barrier_b = Arc::clone(&barrier);

        let worker_a = thread::spawn(move || {
            barrier_a.wait();
            store_a
                .claim(ClaimOutboxMessages::new(
                    "worker-a",
                    1,
                    Duration::from_secs(60),
                ))
                .unwrap()
                .len()
        });
        let worker_b = thread::spawn(move || {
            barrier_b.wait();
            store_b
                .claim(ClaimOutboxMessages::new(
                    "worker-b",
                    1,
                    Duration::from_secs(60),
                ))
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

    #[test]
    fn publish_failure_releases_until_retry_ceiling_then_fails() {
        let repo = HashMapRepository::new();
        let message = OutboxMessage::create("msg-1", "Event", b"{}".to_vec()).unwrap();
        let id = store_message(&repo, message);

        let store = repo.outbox_store();
        let claimed = store
            .claim(ClaimOutboxMessages::new(
                "worker-1",
                1,
                Duration::from_secs(60),
            ))
            .unwrap();
        let claim = OutboxClaimRef::from_message(&claimed[0]).unwrap();
        let action = store.record_failure(&claim, "first failure", 2).unwrap();
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
            .unwrap();
        let claim = OutboxClaimRef::from_message(&claimed[0]).unwrap();
        let action = store.record_failure(&claim, "second failure", 2).unwrap();
        assert_eq!(action, OutboxPublishFailureAction::Failed);

        let stored = load_message(&repo, &id);
        assert!(stored.is_failed());
        assert_eq!(stored.attempts, 2);
        assert_eq!(stored.last_error.as_deref(), Some("second failure"));
    }

    #[test]
    fn missing_message_updates_return_not_found() {
        let store = HashMapOutboxStore {
            storage: Default::default(),
        };
        let expected = RepositoryError::NotFound {
            id: "missing".into(),
        };
        let claim = OutboxClaimRef {
            message_id: "missing".into(),
            worker_id: "worker-1".into(),
            leased_until: SystemTime::now(),
            attempt: 1,
        };

        assert_eq!(store.complete(&claim).unwrap_err(), expected);
        assert_eq!(store.release(&claim, "error").unwrap_err(), expected);
        assert_eq!(store.fail(&claim, "error").unwrap_err(), expected);
    }

    #[test]
    fn stale_or_mismatched_claims_cannot_be_completed() {
        let repo = HashMapRepository::new();
        let message = OutboxMessage::create("msg-1", "Event", b"{}".to_vec()).unwrap();
        let _id = store_message(&repo, message);

        let store = repo.outbox_store();
        let claimed = store
            .claim(ClaimOutboxMessages::new(
                "worker-1",
                1,
                Duration::from_secs(60),
            ))
            .unwrap();
        let mut claim = OutboxClaimRef::from_message(&claimed[0]).unwrap();
        claim.worker_id = "worker-2".into();
        let err = store.complete(&claim).unwrap_err();
        assert!(matches!(err, RepositoryError::InvalidState { .. }));

        let mut expired = OutboxMessage::create("msg-2", "Event", b"{}".to_vec()).unwrap();
        expired
            .claim_at("worker-1", Duration::from_secs(1), SystemTime::UNIX_EPOCH)
            .unwrap();
        let expired_id = store_message(&repo, expired);
        let expired = load_message(&repo, &expired_id);
        let claim = OutboxClaimRef::from_message(&expired).unwrap();
        let err = store.complete(&claim).unwrap_err();
        assert!(matches!(err, RepositoryError::InvalidState { .. }));
    }

    #[test]
    fn stale_attempt_claims_cannot_complete_later_claims() {
        let repo = HashMapRepository::new();
        let message = OutboxMessage::create("msg-1", "Event", b"{}".to_vec()).unwrap();
        let _id = store_message(&repo, message);

        let store = repo.outbox_store();
        let claimed = store
            .claim(ClaimOutboxMessages::new(
                "worker-1",
                1,
                Duration::from_secs(60),
            ))
            .unwrap();
        let stale_claim = OutboxClaimRef::from_message(&claimed[0]).unwrap();
        store.release(&stale_claim, "retry").unwrap();

        let claimed = store
            .claim(ClaimOutboxMessages::new(
                "worker-1",
                1,
                Duration::from_secs(60),
            ))
            .unwrap();
        let current_claim = OutboxClaimRef::from_message(&claimed[0]).unwrap();

        let err = store.complete(&stale_claim).unwrap_err();
        assert!(matches!(err, RepositoryError::InvalidState { .. }));
        store.complete(&current_claim).unwrap();
    }

    #[test]
    fn already_published_message_is_not_completed_again() {
        let repo = HashMapRepository::new();
        let message = OutboxMessage::create("msg-1", "Event", b"{}".to_vec()).unwrap();
        let _id = store_message(&repo, message);

        let store = repo.outbox_store();
        let claimed = store
            .claim(ClaimOutboxMessages::new(
                "worker-1",
                1,
                Duration::from_secs(60),
            ))
            .unwrap();
        let claim = OutboxClaimRef::from_message(&claimed[0]).unwrap();
        store.complete(&claim).unwrap();

        let err = store.complete(&claim).unwrap_err();
        assert!(matches!(err, RepositoryError::InvalidState { .. }));
    }
}
