use std::time::Duration;

use crate::core::{hydrate, Entity, RepositoryError};
use crate::hashmap::HashMapRepository;
use crate::outbox::{OutboxMessage, OutboxMessageStatus};

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

    /// Mark an outbox message as completed (published).
    fn complete_outbox_message(&self, message_id: &str) -> Result<(), RepositoryError>;

    /// Release an outbox message back to pending (for retry).
    fn release_outbox_message(&self, message_id: &str, error: &str) -> Result<(), RepositoryError>;

    /// Mark an outbox message as permanently failed.
    fn fail_outbox_message(&self, message_id: &str, error: &str) -> Result<(), RepositoryError>;
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
                message.claim_for(worker_id, lease);
                *events = message.entity.events().to_vec();
                claimed.push(message);
            }

            if claimed.len() >= max {
                break;
            }
        }

        Ok(claimed)
    }

    fn complete_outbox_message(&self, message_id: &str) -> Result<(), RepositoryError> {
        let normalized_id = if message_id.starts_with(OutboxMessage::ID_PREFIX) {
            message_id.to_string()
        } else {
            format!("{}{}", OutboxMessage::ID_PREFIX, message_id)
        };

        let mut storage = self
            .storage()
            .write()
            .map_err(|_| RepositoryError::LockPoisoned("write"))?;

        if let Some(events) = storage.get_mut(&normalized_id) {
            let mut entity = Entity::with_id(normalized_id);
            entity.load_from_history(events.clone());
            let mut message = hydrate::<OutboxMessage>(entity)?;

            if message.is_in_flight() {
                message.complete();
                *events = message.entity.events().to_vec();
            }
        }

        Ok(())
    }

    fn release_outbox_message(&self, message_id: &str, error: &str) -> Result<(), RepositoryError> {
        let normalized_id = if message_id.starts_with(OutboxMessage::ID_PREFIX) {
            message_id.to_string()
        } else {
            format!("{}{}", OutboxMessage::ID_PREFIX, message_id)
        };

        let mut storage = self
            .storage()
            .write()
            .map_err(|_| RepositoryError::LockPoisoned("write"))?;

        if let Some(events) = storage.get_mut(&normalized_id) {
            let mut entity = Entity::with_id(normalized_id);
            entity.load_from_history(events.clone());
            let mut message = hydrate::<OutboxMessage>(entity)?;

            if message.is_in_flight() {
                message.release(error.to_string());
                *events = message.entity.events().to_vec();
            }
        }

        Ok(())
    }

    fn fail_outbox_message(&self, message_id: &str, error: &str) -> Result<(), RepositoryError> {
        let normalized_id = if message_id.starts_with(OutboxMessage::ID_PREFIX) {
            message_id.to_string()
        } else {
            format!("{}{}", OutboxMessage::ID_PREFIX, message_id)
        };

        let mut storage = self
            .storage()
            .write()
            .map_err(|_| RepositoryError::LockPoisoned("write"))?;

        if let Some(events) = storage.get_mut(&normalized_id) {
            let mut entity = Entity::with_id(normalized_id);
            entity.load_from_history(events.clone());
            let mut message = hydrate::<OutboxMessage>(entity)?;

            message.fail(error.to_string());
            *events = message.entity.events().to_vec();
        }

        Ok(())
    }
}
