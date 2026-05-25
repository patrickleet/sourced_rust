#![expect(
    clippy::manual_async_fn,
    reason = "async trait impls return impl Future + Send to preserve public Send bounds"
)]

use std::future::Future;
use std::time::{Duration, SystemTime};

use crate::aggregate::hydrate;
use crate::entity::{Entity, EventRecord};
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

    /// Administrative completion path that does not verify a worker ID.
    ///
    /// Worker loops should use [`Self::complete_outbox_message_for_worker`]
    /// so stale or stolen leases cannot complete messages claimed by another
    /// worker.
    #[deprecated(
        note = "worker code should use complete_outbox_message_for_worker to validate the active claim"
    )]
    fn complete_outbox_message(&self, message_id: &str) -> Result<(), RepositoryError>;

    /// Mark an outbox message as completed if it is still claimed by this worker.
    fn complete_outbox_message_for_worker(
        &self,
        message_id: &str,
        worker_id: &str,
    ) -> Result<(), RepositoryError>;

    /// Administrative release path that does not verify a worker ID.
    ///
    /// Worker loops should use [`Self::release_outbox_message_for_worker`] or
    /// [`Self::record_outbox_publish_failure`] so stale or stolen leases cannot
    /// release messages claimed by another worker.
    #[deprecated(
        note = "worker code should use release_outbox_message_for_worker or record_outbox_publish_failure to validate the active claim"
    )]
    fn release_outbox_message(&self, message_id: &str, error: &str) -> Result<(), RepositoryError>;

    /// Release an outbox message if it is still claimed by this worker.
    fn release_outbox_message_for_worker(
        &self,
        message_id: &str,
        worker_id: &str,
        error: &str,
    ) -> Result<(), RepositoryError>;

    /// Mark an outbox message as permanently failed.
    fn fail_outbox_message(&self, message_id: &str, error: &str) -> Result<(), RepositoryError>;

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

fn normalize_outbox_id(message_id: &str) -> String {
    if message_id.starts_with(OutboxMessage::ID_PREFIX) {
        message_id.to_string()
    } else {
        format!("{}{}", OutboxMessage::ID_PREFIX, message_id)
    }
}

fn hydrate_outbox_message(
    normalized_id: &str,
    events: &[EventRecord],
) -> Result<OutboxMessage, RepositoryError> {
    let mut entity = Entity::with_id(normalized_id.to_string());
    entity.load_from_history(events.to_vec());
    hydrate::<OutboxMessage>(entity)
}

fn persist_outbox_message(events: &mut Vec<EventRecord>, message: &mut OutboxMessage) {
    *events = message.entity.events().to_vec();
    message.entity.mark_committed();
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

fn ensure_active_claim(
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
        let normalized_id = normalize_outbox_id(message_id);
        let mut storage = self
            .event_store()
            .write()
            .map_err(|_| RepositoryError::LockPoisoned("write"))?;

        let events = storage
            .get_mut(&normalized_id)
            .ok_or_else(|| RepositoryError::NotFound {
                id: normalized_id.clone(),
            })?;

        let mut message = hydrate_outbox_message(&normalized_id, events)?;
        let result = update(&mut message)?;
        persist_outbox_message(events, &mut message);
        Ok(result)
    }
}

impl OutboxRepositoryExt for HashMapRepository {
    fn outbox_messages_by_status(
        &self,
        status: OutboxMessageStatus,
    ) -> Result<Vec<OutboxMessage>, RepositoryError> {
        let storage = self
            .event_store()
            .read()
            .map_err(|_| RepositoryError::LockPoisoned("read"))?;

        let mut messages = Vec::new();
        for (id, events) in storage.iter() {
            if !id.starts_with(OutboxMessage::ID_PREFIX) {
                continue;
            }

            let message = hydrate_outbox_message(id, events)?;

            if message.status == status {
                messages.push(message);
            }
        }

        Ok(messages)
    }

    fn claim_outbox_messages(
        &self,
        worker_id: &str,
        max: usize,
        lease: Duration,
    ) -> Result<Vec<OutboxMessage>, RepositoryError> {
        let mut storage = self
            .event_store()
            .write()
            .map_err(|_| RepositoryError::LockPoisoned("write"))?;

        if max == 0 {
            return Ok(Vec::new());
        }

        let now = SystemTime::now();
        let mut claimed = Vec::new();
        for (id, events) in storage.iter_mut() {
            if !id.starts_with(OutboxMessage::ID_PREFIX) {
                continue;
            }

            let mut message = hydrate_outbox_message(id, events)?;

            if message.is_claimable_at(now) {
                message.claim_at(worker_id, lease, now)?;
                persist_outbox_message(events, &mut message);
                claimed.push(message);
            }

            if claimed.len() >= max {
                break;
            }
        }

        Ok(claimed)
    }

    fn complete_outbox_message(&self, message_id: &str) -> Result<(), RepositoryError> {
        self.update_outbox_message(message_id, |message| {
            ensure_active_claim(message, None, SystemTime::now())?;
            message.complete()?;
            Ok(())
        })
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

    fn release_outbox_message(&self, message_id: &str, error: &str) -> Result<(), RepositoryError> {
        self.update_outbox_message(message_id, |message| {
            ensure_active_claim(message, None, SystemTime::now())?;
            message.release(error.to_string())?;
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

    fn fail_outbox_message(&self, message_id: &str, error: &str) -> Result<(), RepositoryError> {
        self.update_outbox_message(message_id, |message| {
            if message.is_published() || message.is_failed() {
                return Err(invalid_outbox_state(
                    message,
                    "outbox message that can be failed",
                ));
            }
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
        async move { OutboxRepositoryExt::outbox_messages_by_status(self, status) }
    }

    fn claim_outbox_messages_async<'a>(
        &'a self,
        worker_id: &'a str,
        max: usize,
        lease: Duration,
    ) -> impl Future<Output = Result<Vec<OutboxMessage>, RepositoryError>> + Send + 'a {
        async move { OutboxRepositoryExt::claim_outbox_messages(self, worker_id, max, lease) }
    }

    fn complete_outbox_message_for_worker_async<'a>(
        &'a self,
        message_id: &'a str,
        worker_id: &'a str,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        async move {
            OutboxRepositoryExt::complete_outbox_message_for_worker(self, message_id, worker_id)
        }
    }

    fn release_outbox_message_for_worker_async<'a>(
        &'a self,
        message_id: &'a str,
        worker_id: &'a str,
        error: &'a str,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        async move {
            OutboxRepositoryExt::release_outbox_message_for_worker(
                self, message_id, worker_id, error,
            )
        }
    }

    fn fail_outbox_message_async<'a>(
        &'a self,
        message_id: &'a str,
        error: &'a str,
    ) -> impl Future<Output = Result<(), RepositoryError>> + Send + 'a {
        async move { OutboxRepositoryExt::fail_outbox_message(self, message_id, error) }
    }

    fn record_outbox_publish_failure_async<'a>(
        &'a self,
        message_id: &'a str,
        worker_id: &'a str,
        error: &'a str,
        max_attempts: u32,
    ) -> impl Future<Output = Result<OutboxPublishFailureAction, RepositoryError>> + Send + 'a {
        async move {
            OutboxRepositoryExt::record_outbox_publish_failure(
                self,
                message_id,
                worker_id,
                error,
                max_attempts,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::GetAggregate;
    use crate::repository::Commit;
    use std::sync::{Arc, Barrier};
    use std::thread;

    fn store_message(repo: &HashMapRepository, message: &mut OutboxMessage) -> String {
        let id = message.id().to_string();
        repo.commit(&mut message.entity).unwrap();
        id
    }

    fn load_message(repo: &HashMapRepository, id: &str) -> OutboxMessage {
        repo.get_aggregate::<OutboxMessage>(id).unwrap().unwrap()
    }

    #[test]
    fn claim_includes_expired_in_flight_messages() {
        let repo = HashMapRepository::new();
        let mut message = OutboxMessage::create("msg-1", "Event", b"{}".to_vec()).unwrap();
        message
            .claim_at("worker-1", Duration::from_secs(1), SystemTime::UNIX_EPOCH)
            .unwrap();
        let id = store_message(&repo, &mut message);

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
        let id = store_message(&repo, &mut message);

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
        let mut message = OutboxMessage::create("msg-1", "Event", b"{}".to_vec()).unwrap();
        let id = store_message(&repo, &mut message);

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
        let mut message = OutboxMessage::create("msg-1", "Event", b"{}".to_vec()).unwrap();
        let id = store_message(&repo, &mut message);

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
    #[allow(deprecated)]
    fn missing_message_updates_return_not_found() {
        let repo = HashMapRepository::new();
        let expected = RepositoryError::NotFound {
            id: "outbox:missing".into(),
        };

        assert_eq!(
            repo.complete_outbox_message("missing").unwrap_err(),
            expected
        );
        assert_eq!(
            repo.release_outbox_message("missing", "error").unwrap_err(),
            expected
        );
        assert_eq!(
            repo.fail_outbox_message("missing", "error").unwrap_err(),
            expected
        );
    }

    #[test]
    fn stale_or_mismatched_claims_cannot_be_completed() {
        let repo = HashMapRepository::new();
        let mut message = OutboxMessage::create("msg-1", "Event", b"{}".to_vec()).unwrap();
        let id = store_message(&repo, &mut message);

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
        let expired_id = store_message(&repo, &mut expired);
        let err = repo
            .complete_outbox_message_for_worker(&expired_id, "worker-1")
            .unwrap_err();
        assert!(matches!(err, RepositoryError::InvalidState { .. }));
    }

    #[test]
    fn already_published_message_is_not_completed_again() {
        let repo = HashMapRepository::new();
        let mut message = OutboxMessage::create("msg-1", "Event", b"{}".to_vec()).unwrap();
        let id = store_message(&repo, &mut message);

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
