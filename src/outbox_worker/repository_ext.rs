#![expect(
    clippy::manual_async_fn,
    reason = "async trait impls return impl Future + Send to preserve public Send bounds"
)]

use std::future::Future;
use std::time::{Duration, SystemTime};

use crate::hashmap_repo::HashMapRepository;
use crate::outbox::{OutboxMessage, OutboxMessageStatus};
use crate::repository::{AsyncOutboxRepositoryExt, RepositoryError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboxPublishFailureAction {
    Released,
    Failed,
}

/// Extension trait for repositories that expose outbox message operations.
pub trait OutboxRepositoryExt: Send + Sync {
    /// Return all outbox messages with the given status.
    fn outbox_messages_by_status(
        &self,
        status: OutboxMessageStatus,
    ) -> Result<Vec<OutboxMessage>, RepositoryError>;

    /// Return all pending outbox messages.
    fn outbox_messages_pending(&self) -> Result<Vec<OutboxMessage>, RepositoryError> {
        self.outbox_messages_by_status(OutboxMessageStatus::Pending)
    }

    /// Claim pending outbox messages for processing.
    fn claim_outbox_messages(
        &self,
        worker_id: &str,
        max: usize,
        lease: Duration,
    ) -> Result<Vec<OutboxMessage>, RepositoryError>;

    /// Mark an outbox message as completed if it is still claimed by this worker.
    fn complete_outbox_message_for_worker(
        &self,
        message_id: &str,
        worker_id: &str,
    ) -> Result<(), RepositoryError>;

    /// Release an outbox message if it is still claimed by this worker.
    fn release_outbox_message_for_worker(
        &self,
        message_id: &str,
        worker_id: &str,
        error: &str,
    ) -> Result<(), RepositoryError>;

    /// Mark an outbox message as permanently failed if it is still claimed by this worker.
    fn fail_outbox_message_for_worker(
        &self,
        message_id: &str,
        worker_id: &str,
        error: &str,
    ) -> Result<(), RepositoryError>;

    /// Record a publish failure for a claimed message, releasing it for retry
    /// or permanently failing it when the attempt ceiling is reached.
    fn record_outbox_publish_failure(
        &self,
        message_id: &str,
        worker_id: &str,
        error: &str,
        max_attempts: u32,
    ) -> Result<OutboxPublishFailureAction, RepositoryError>;
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
    worker_id: Option<&str>,
    now: SystemTime,
) -> Result<(), RepositoryError> {
    if !message.is_in_flight() {
        return Err(invalid_outbox_state(message, "in-flight outbox message"));
    }

    if let Some(worker_id) = worker_id {
        if !message.is_claimed_by(worker_id) {
            return Err(invalid_outbox_state(
                message,
                "outbox claim held by requesting worker",
            ));
        }
    }

    if message.has_expired_lease_at(now) {
        return Err(invalid_outbox_state(message, "unexpired outbox claim"));
    }

    Ok(())
}

impl HashMapRepository {
    fn update_outbox_message<T>(
        &self,
        message_id: &str,
        update: impl FnOnce(&mut OutboxMessage) -> Result<T, RepositoryError>,
    ) -> Result<T, RepositoryError> {
        let mut storage = self
            .outbox_store()
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

impl OutboxRepositoryExt for HashMapRepository {
    fn outbox_messages_by_status(
        &self,
        status: OutboxMessageStatus,
    ) -> Result<Vec<OutboxMessage>, RepositoryError> {
        let storage = self
            .outbox_store()
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

    fn claim_outbox_messages(
        &self,
        worker_id: &str,
        max: usize,
        lease: Duration,
    ) -> Result<Vec<OutboxMessage>, RepositoryError> {
        let mut storage = self
            .outbox_store()
            .write()
            .map_err(|_| RepositoryError::LockPoisoned("outbox write"))?;

        if max == 0 {
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
                message.claim_at(worker_id, lease, now)?;
                claimed.push(message.clone());
            }

            if claimed.len() >= max {
                break;
            }
        }

        Ok(claimed)
    }

    fn complete_outbox_message_for_worker(
        &self,
        message_id: &str,
        worker_id: &str,
    ) -> Result<(), RepositoryError> {
        self.update_outbox_message(message_id, |message| {
            ensure_active_claim(message, Some(worker_id), SystemTime::now())?;
            message.complete()?;
            Ok(())
        })
    }

    fn release_outbox_message_for_worker(
        &self,
        message_id: &str,
        worker_id: &str,
        error: &str,
    ) -> Result<(), RepositoryError> {
        self.update_outbox_message(message_id, |message| {
            ensure_active_claim(message, Some(worker_id), SystemTime::now())?;
            message.release(error.to_string())?;
            Ok(())
        })
    }

    fn fail_outbox_message_for_worker(
        &self,
        message_id: &str,
        worker_id: &str,
        error: &str,
    ) -> Result<(), RepositoryError> {
        self.update_outbox_message(message_id, |message| {
            ensure_active_claim(message, Some(worker_id), SystemTime::now())?;
            message.fail(error.to_string())?;
            Ok(())
        })
    }

    fn record_outbox_publish_failure(
        &self,
        message_id: &str,
        worker_id: &str,
        error: &str,
        max_attempts: u32,
    ) -> Result<OutboxPublishFailureAction, RepositoryError> {
        self.update_outbox_message(message_id, |message| {
            ensure_active_claim(message, Some(worker_id), SystemTime::now())?;
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

impl AsyncOutboxRepositoryExt for HashMapRepository {
    fn outbox_messages_by_status_async(
        &self,
        status: OutboxMessageStatus,
    ) -> impl Future<Output = Result<Vec<OutboxMessage>, RepositoryError>> + Send + '_ {
        async move {
            let storage = self
                .outbox_store()
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

    fn claim_outbox_messages_async<'a>(
        &'a self,
        worker_id: &'a str,
        max: usize,
        lease: Duration,
    ) -> impl Future<Output = Result<Vec<OutboxMessage>, RepositoryError>> + Send + 'a {
        async move {
            if max == 0 {
                return Ok(Vec::new());
            }

            let now = SystemTime::now();
            let mut storage = self
                .outbox_store()
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

                message.claim_at(worker_id, lease, now)?;
                claimed.push(message.clone());

                if claimed.len() >= max {
                    break;
                }
            }

            Ok(claimed)
        }
    }

    fn complete_outbox_message_for_worker_async<'a>(
        &'a self,
        message_id: &'a str,
        worker_id: &'a str,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        async move {
            let mut storage = self
                .outbox_store()
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("async outbox write"))?;
            let message = storage
                .get_mut(message_id)
                .ok_or_else(|| RepositoryError::NotFound {
                    id: message_id.to_string(),
                })?;
            ensure_active_claim(message, Some(worker_id), SystemTime::now())?;
            message.complete()?;
            Ok(())
        }
    }

    fn release_outbox_message_for_worker_async<'a>(
        &'a self,
        message_id: &'a str,
        worker_id: &'a str,
        error: &'a str,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        async move {
            let mut storage = self
                .outbox_store()
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("async outbox write"))?;
            let message = storage
                .get_mut(message_id)
                .ok_or_else(|| RepositoryError::NotFound {
                    id: message_id.to_string(),
                })?;
            ensure_active_claim(message, Some(worker_id), SystemTime::now())?;
            message.release(error.to_string())?;
            Ok(())
        }
    }

    fn fail_outbox_message_for_worker_async<'a>(
        &'a self,
        message_id: &'a str,
        worker_id: &'a str,
        error: &'a str,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        async move {
            let mut storage = self
                .outbox_store()
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("async outbox write"))?;
            let message = storage
                .get_mut(message_id)
                .ok_or_else(|| RepositoryError::NotFound {
                    id: message_id.to_string(),
                })?;
            ensure_active_claim(message, Some(worker_id), SystemTime::now())?;
            message.fail(error.to_string())?;
            Ok(())
        }
    }

    fn record_outbox_publish_failure_async<'a>(
        &'a self,
        message_id: &'a str,
        worker_id: &'a str,
        error: &'a str,
        max_attempts: u32,
    ) -> impl Future<Output = Result<OutboxPublishFailureAction, RepositoryError>> + Send + 'a {
        async move {
            let mut storage = self
                .outbox_store()
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("async outbox write"))?;
            let message = storage
                .get_mut(message_id)
                .ok_or_else(|| RepositoryError::NotFound {
                    id: message_id.to_string(),
                })?;
            ensure_active_claim(message, Some(worker_id), SystemTime::now())?;
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
    use crate::TransactionalCommit;
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
        repo.outbox_store().read().unwrap().get(id).unwrap().clone()
    }

    #[test]
    fn claim_includes_expired_in_flight_messages() {
        let repo = HashMapRepository::new();
        let mut message = OutboxMessage::create("msg-1", "Event", b"{}".to_vec()).unwrap();
        message
            .claim_at("worker-1", Duration::from_secs(1), SystemTime::UNIX_EPOCH)
            .unwrap();
        let id = store_message(&repo, message);

        let claimed = repo
            .claim_outbox_messages("worker-2", 1, Duration::from_secs(60))
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

        let claimed = repo
            .claim_outbox_messages("worker-2", 1, Duration::from_secs(60))
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
        let repo_a = repo.clone();
        let repo_b = repo.clone();
        let barrier_a = Arc::clone(&barrier);
        let barrier_b = Arc::clone(&barrier);

        let worker_a = thread::spawn(move || {
            barrier_a.wait();
            repo_a
                .claim_outbox_messages("worker-a", 1, Duration::from_secs(60))
                .unwrap()
                .len()
        });
        let worker_b = thread::spawn(move || {
            barrier_b.wait();
            repo_b
                .claim_outbox_messages("worker-b", 1, Duration::from_secs(60))
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

        repo.claim_outbox_messages("worker-1", 1, Duration::from_secs(60))
            .unwrap();
        let action = repo
            .record_outbox_publish_failure(&id, "worker-1", "first failure", 2)
            .unwrap();
        assert_eq!(action, OutboxPublishFailureAction::Released);

        let stored = load_message(&repo, &id);
        assert!(stored.is_pending());
        assert_eq!(stored.attempts, 1);
        assert_eq!(stored.last_error.as_deref(), Some("first failure"));

        repo.claim_outbox_messages("worker-1", 1, Duration::from_secs(60))
            .unwrap();
        let action = repo
            .record_outbox_publish_failure(&id, "worker-1", "second failure", 2)
            .unwrap();
        assert_eq!(action, OutboxPublishFailureAction::Failed);

        let stored = load_message(&repo, &id);
        assert!(stored.is_failed());
        assert_eq!(stored.attempts, 2);
        assert_eq!(stored.last_error.as_deref(), Some("second failure"));
    }

    #[test]
    fn missing_message_updates_return_not_found() {
        let repo = HashMapRepository::new();
        let expected = RepositoryError::NotFound {
            id: "missing".into(),
        };

        assert_eq!(
            repo.complete_outbox_message_for_worker("missing", "worker-1")
                .unwrap_err(),
            expected
        );
        assert_eq!(
            repo.release_outbox_message_for_worker("missing", "worker-1", "error")
                .unwrap_err(),
            expected
        );
        assert_eq!(
            repo.fail_outbox_message_for_worker("missing", "worker-1", "error")
                .unwrap_err(),
            expected
        );
    }

    #[test]
    fn stale_or_mismatched_claims_cannot_be_completed() {
        let repo = HashMapRepository::new();
        let message = OutboxMessage::create("msg-1", "Event", b"{}".to_vec()).unwrap();
        let id = store_message(&repo, message);

        repo.claim_outbox_messages("worker-1", 1, Duration::from_secs(60))
            .unwrap();
        let err = repo
            .complete_outbox_message_for_worker(&id, "worker-2")
            .unwrap_err();
        assert!(matches!(err, RepositoryError::InvalidState { .. }));

        let mut expired = OutboxMessage::create("msg-2", "Event", b"{}".to_vec()).unwrap();
        expired
            .claim_at("worker-1", Duration::from_secs(1), SystemTime::UNIX_EPOCH)
            .unwrap();
        let expired_id = store_message(&repo, expired);
        let err = repo
            .complete_outbox_message_for_worker(&expired_id, "worker-1")
            .unwrap_err();
        assert!(matches!(err, RepositoryError::InvalidState { .. }));
    }

    #[test]
    fn already_published_message_is_not_completed_again() {
        let repo = HashMapRepository::new();
        let message = OutboxMessage::create("msg-1", "Event", b"{}".to_vec()).unwrap();
        let id = store_message(&repo, message);

        repo.claim_outbox_messages("worker-1", 1, Duration::from_secs(60))
            .unwrap();
        repo.complete_outbox_message_for_worker(&id, "worker-1")
            .unwrap();

        let err = repo
            .complete_outbox_message_for_worker(&id, "worker-1")
            .unwrap_err();
        assert!(matches!(err, RepositoryError::InvalidState { .. }));
    }
}
