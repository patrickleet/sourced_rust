use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use crate::entity::Entity;
use crate::event_record::EventRecord;
use crate::error::RepositoryError;
use crate::outbox::{OutboxRecord, OutboxRepository, OutboxStatus};
use crate::Repository;

pub struct HashMapRepository {
    storage: Arc<RwLock<HashMap<String, Vec<EventRecord>>>>,
    outbox: Arc<RwLock<Vec<OutboxRecord>>>,
    outbox_seq: AtomicU64,
}

impl HashMapRepository {
    pub fn new() -> Self {
        HashMapRepository {
            storage: Arc::new(RwLock::new(HashMap::new())),
            outbox: Arc::new(RwLock::new(Vec::new())),
            outbox_seq: AtomicU64::new(1),
        }
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

    fn commit(&self, entity: &mut Entity) -> Result<(), RepositoryError> {
        {
            let mut storage = self
                .storage
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("write"))?;
            let mut outbox = self
                .outbox
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("outbox write"))?;
            storage.insert(entity.id().to_string(), entity.events().to_vec());
            append_outbox(
                &self.outbox_seq,
                &mut outbox,
                entity.id(),
                entity.version(),
                entity.outbox_events(),
            );
            entity.clear_outbox();
        }
        entity.emit_queued_events();

        Ok(())
    }

    fn commit_all(&self, entities: &mut [&mut Entity]) -> Result<(), RepositoryError> {
        {
            let mut storage = self
                .storage
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("write"))?;
            let mut outbox = self
                .outbox
                .write()
                .map_err(|_| RepositoryError::LockPoisoned("outbox write"))?;

            for entity in entities.iter_mut() {
                storage.insert(entity.id().to_string(), entity.events().to_vec());
                append_outbox(
                    &self.outbox_seq,
                    &mut outbox,
                    entity.id(),
                    entity.version(),
                    entity.outbox_events(),
                );
                entity.clear_outbox();
            }
        }

        for entity in entities.iter_mut() {
            entity.emit_queued_events();
        }

        Ok(())
    }
}

impl OutboxRepository for HashMapRepository {
    fn peek_outbox(&self) -> Result<Vec<OutboxRecord>, RepositoryError> {
        let outbox = self
            .outbox
            .read()
            .map_err(|_| RepositoryError::LockPoisoned("outbox read"))?;
        Ok(outbox.clone())
    }

    fn claim_outbox(
        &self,
        worker_id: &str,
        max: usize,
        lease: std::time::Duration,
    ) -> Result<Vec<OutboxRecord>, RepositoryError> {
        let mut outbox = self
            .outbox
            .write()
            .map_err(|_| RepositoryError::LockPoisoned("outbox write"))?;
        let now = std::time::SystemTime::now();
        let mut claimed = Vec::new();

        for record in outbox.iter_mut() {
            if claimed.len() >= max {
                break;
            }

            if record.status == OutboxStatus::Published || record.status == OutboxStatus::Failed {
                continue;
            }

            let expired = record
                .locked_until
                .map(|until| until <= now)
                .unwrap_or(true);

            let available = record.status == OutboxStatus::Pending
                || (record.status == OutboxStatus::InFlight && expired);

            if !available {
                continue;
            }

            let locked_until = now.checked_add(lease).unwrap_or(now);
            record.status = OutboxStatus::InFlight;
            record.attempts = record.attempts.saturating_add(1);
            record.locked_by = Some(worker_id.to_string());
            record.locked_until = Some(locked_until);
            record.last_error = None;
            claimed.push(record.clone());
        }

        Ok(claimed)
    }

    fn complete_outbox(&self, ids: &[u64]) -> Result<(), RepositoryError> {
        let mut outbox = self
            .outbox
            .write()
            .map_err(|_| RepositoryError::LockPoisoned("outbox write"))?;
        let now = std::time::SystemTime::now();

        for record in outbox.iter_mut() {
            if ids.contains(&record.id) {
                record.status = OutboxStatus::Published;
                record.published_at = Some(now);
                record.locked_by = None;
                record.locked_until = None;
                record.last_error = None;
            }
        }

        Ok(())
    }

    fn release_outbox(&self, ids: &[u64], error: Option<&str>) -> Result<(), RepositoryError> {
        let mut outbox = self
            .outbox
            .write()
            .map_err(|_| RepositoryError::LockPoisoned("outbox write"))?;
        for record in outbox.iter_mut() {
            if ids.contains(&record.id) {
                record.status = OutboxStatus::Pending;
                record.locked_by = None;
                record.locked_until = None;
                record.last_error = error.map(|value| value.to_string());
            }
        }

        Ok(())
    }

    fn fail_outbox(&self, ids: &[u64], error: Option<&str>) -> Result<(), RepositoryError> {
        let mut outbox = self
            .outbox
            .write()
            .map_err(|_| RepositoryError::LockPoisoned("outbox write"))?;
        let now = std::time::SystemTime::now();
        for record in outbox.iter_mut() {
            if ids.contains(&record.id) {
                record.status = OutboxStatus::Failed;
                record.failed_at = Some(now);
                record.locked_by = None;
                record.locked_until = None;
                record.last_error = error.map(|value| value.to_string());
            }
        }

        Ok(())
    }
}

fn append_outbox(
    seq: &AtomicU64,
    outbox: &mut Vec<OutboxRecord>,
    aggregate_id: &str,
    aggregate_version: u64,
    events: &[crate::entity::OutboxEvent],
) {
    if events.is_empty() {
        return;
    }

    for event in events {
        let id = seq.fetch_add(1, Ordering::Relaxed);
        outbox.push(OutboxRecord {
            id,
            aggregate_id: aggregate_id.to_string(),
            aggregate_version,
            event_type: event.event_type.clone(),
            payload: event.payload.clone(),
            occurred_at: std::time::SystemTime::now(),
            status: OutboxStatus::Pending,
            attempts: 0,
            locked_by: None,
            locked_until: None,
            published_at: None,
            failed_at: None,
            last_error: None,
        });
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
    fn full_workflow() {
        let repo = HashMapRepository::new();
        let id = "test_id";
        let mut entity = Entity::with_id(id);

        let args = vec!["arg1".to_string(), "arg2".to_string()];
        entity.digest("test_event", args);

        entity.enqueue("test_event", "test_data");

        entity.on("test_event", |data| {
            assert!(data == "test_data");
        });

        repo.commit(&mut entity).unwrap();

        let fetched_entity = repo.get(id).unwrap().unwrap();
        assert_eq!(fetched_entity.id(), id);
        assert_eq!(fetched_entity.events(), entity.events());

        let args2 = vec!["arg1".to_string(), "arg2".to_string()];

        let mut entity2 = Entity::with_id("test_id_2");
        entity2.digest("test_event", args2);

        let result = repo.commit_all(&mut [&mut entity, &mut entity2]);
        assert!(result.is_ok());

        let all_entities = repo.get_all(&[id, "test_id_2"]).unwrap();
        assert_eq!(all_entities.len(), 2);
    }
}
