use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use std::time::Duration;

use crate::core::{hydrate, Committable, Entity, EventRecord, Repository, RepositoryError};
use crate::outbox::{OutboxMessage, OutboxMessageStatus};

/// In-memory repository implementation using HashMap.
///
/// This is a simple storage implementation suitable for testing and prototyping.
/// For outbox support, commit `OutboxMessage` entities alongside your entity:
///
/// ```ignore
/// use sourced_rust::{OutboxMessage, HashMapRepository, Repository};
///
/// let repo = HashMapRepository::new();
/// let mut message = OutboxMessage::new("msg-1", "TodoInitialized", "{}");
/// // ... mutate domain entity and message ...
/// repo.commit(&mut [&mut entity, &mut message.entity])?;
/// ```
pub struct HashMapRepository {
    storage: Arc<RwLock<HashMap<String, Vec<EventRecord>>>>,
}

impl Default for HashMapRepository {
    fn default() -> Self {
        Self::new()
    }
}

impl HashMapRepository {
    /// Create a new empty repository.
    pub fn new() -> Self {
        HashMapRepository {
            storage: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Return all outbox messages with the given status.
    pub fn outbox_messages_by_status(
        &self,
        status: OutboxMessageStatus,
    ) -> Result<Vec<OutboxMessage>, RepositoryError> {
        let storage = self
            .storage
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

    /// Return all pending outbox messages.
    pub fn outbox_messages_pending(&self) -> Result<Vec<OutboxMessage>, RepositoryError> {
        self.outbox_messages_by_status(OutboxMessageStatus::Pending)
    }

    /// Claim pending outbox messages for processing.
    ///
    /// This is repo-specific and intended for single-process usage or tests.
    pub fn claim_outbox_messages(
        &self,
        worker_id: &str,
        max: usize,
        lease: Duration,
    ) -> Result<Vec<OutboxMessage>, RepositoryError> {
        let mut storage = self
            .storage
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

impl Repository for HashMapRepository {
    fn get(&self, id: &str) -> Result<Option<Entity>, RepositoryError> {
        let storage = self
            .storage
            .read()
            .map_err(|_| RepositoryError::LockPoisoned("read"))?;

        if let Some(events) = storage.get(id) {
            let mut entity = Entity::new();
            entity.set_id(id);
            entity.load_from_history(events.clone());
            Ok(Some(entity))
        } else {
            Ok(None)
        }
    }

    fn get_all(&self, ids: &[&str]) -> Result<Vec<Entity>, RepositoryError> {
        let mut entities = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(entity) = self.get(id)? {
                entities.push(entity);
            }
        }
        Ok(entities)
    }

    fn commit<C: Committable + ?Sized>(&self, committable: &mut C) -> Result<(), RepositoryError> {
        let entities = committable.entities_mut();

        let mut storage = self
            .storage
            .write()
            .map_err(|_| RepositoryError::LockPoisoned("write"))?;

        for entity in &entities {
            storage.insert(entity.id().to_string(), entity.events().to_vec());
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new() {
        let repo = HashMapRepository::new();
        assert!(repo.storage.read().unwrap().is_empty());
    }

    #[test]
    fn single_entity_commit() {
        let repo = HashMapRepository::new();
        let id = "test_id";
        let mut entity = Entity::with_id(id);

        let args = vec!["arg1".to_string(), "arg2".to_string()];
        entity.digest("test_event", args);

        repo.commit(&mut entity).unwrap();

        let fetched_entity = repo.get(id).unwrap().unwrap();
        assert_eq!(fetched_entity.id(), id);
        assert_eq!(fetched_entity.events(), entity.events());
    }

    #[test]
    fn multiple_entity_commit() {
        let repo = HashMapRepository::new();

        let mut entity1 = Entity::with_id("id_1");
        entity1.digest("event1", vec!["arg1".to_string()]);

        let mut entity2 = Entity::with_id("id_2");
        entity2.digest("event2", vec!["arg2".to_string()]);

        // Commit multiple entities using array syntax
        repo.commit(&mut [&mut entity1, &mut entity2]).unwrap();

        let all_entities = repo.get_all(&["id_1", "id_2"]).unwrap();
        assert_eq!(all_entities.len(), 2);
    }

    #[test]
    fn outbox_messages_pending_finds_messages() {
        let repo = HashMapRepository::new();
        let mut entity = Entity::with_id("test");
        entity.digest("Created", vec!["test".to_string()]);

        let mut message = OutboxMessage::new("msg-1", "EntityCreated", r#"{"id":"test"}"#);
        repo.commit(&mut [&mut entity, &mut message.entity]).unwrap();

        let pending = repo.outbox_messages_pending().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].event_type, "EntityCreated");
    }
}
