use std::time::Duration;

use crate::core::{hydrate, Entity, RepositoryError};
use crate::hashmap::HashMapRepository;
use crate::outbox::{OutboxMessage, OutboxMessageStatus};

/// Extension trait for repositories that expose outbox message operations.
pub trait OutboxRepositoryExt {
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
}

impl OutboxRepositoryExt for HashMapRepository {
    fn outbox_messages_by_status(
        &self,
        status: OutboxMessageStatus,
    ) -> Result<Vec<OutboxMessage>, RepositoryError> {
        let storage = self
            .storage()
            .read()
            .map_err(|_| RepositoryError::LockPoisoned("read"))?;

        let mut messages = Vec::new();
        for (id, events) in storage.iter() {
            if !id.starts_with(OutboxMessage::ID_PREFIX) {
                continue;
            }

            let mut entity = Entity::with_id(id.to_string());
            entity.load_from_history(events.clone());
            let message = hydrate::<OutboxMessage>(entity)?;

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
            .storage()
            .write()
            .map_err(|_| RepositoryError::LockPoisoned("write"))?;

        let mut claimed = Vec::new();
        for (id, events) in storage.iter_mut() {
            if !id.starts_with(OutboxMessage::ID_PREFIX) {
                continue;
            }

            let mut entity = Entity::with_id(id.to_string());
            entity.load_from_history(events.clone());
            let mut message = hydrate::<OutboxMessage>(entity)?;

            if message.is_pending() {
                message.claim(worker_id, lease);
                *events = message.entity.events().to_vec();
                claimed.push(message);
            }

            if claimed.len() >= max {
                break;
            }
        }

        Ok(claimed)
    }
}
