use std::ops::{Deref, DerefMut};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::core::{Committable, Entity, Repository, RepositoryError};

use super::Outbox;

/// Repository wrapper that adds outbox capability to any Repository.
///
/// This wraps an inner repository and manages an Outbox aggregate.
/// When entities are committed, any outbox events are atomically
/// added to the Outbox and persisted.
///
/// # Example
///
/// ```ignore
/// use sourced_rust::{HashMapRepository, OutboxRepository, OutboxEntity};
///
/// let storage = HashMapRepository::new();
/// let repo = OutboxRepository::new(storage);
///
/// let mut entity = OutboxEntity::with_id("123");
/// entity.digest("Created", vec!["123".to_string()]);
/// entity.outbox("EntityCreated", r#"{"id":"123"}"#);
///
/// repo.commit(&mut entity)?;
///
/// // Access the outbox
/// let outbox = repo.outbox();
/// assert_eq!(outbox.pending_count(), 1);
/// ```
pub struct OutboxRepository<R> {
    inner: R,
    outbox: Arc<RwLock<Outbox>>,
}

impl<R> OutboxRepository<R> {
    /// Create a new OutboxRepository wrapping the given repository.
    pub fn new(inner: R) -> Self {
        Self::with_outbox_id(inner, "outbox")
    }

    /// Create a new OutboxRepository with a custom outbox ID.
    pub fn with_outbox_id(inner: R, id: impl Into<String>) -> Self {
        Self {
            inner,
            outbox: Arc::new(RwLock::new(Outbox::new(id))),
        }
    }

    /// Access the inner repository.
    pub fn inner(&self) -> &R {
        &self.inner
    }

    /// Access the inner repository mutably.
    pub fn inner_mut(&mut self) -> &mut R {
        &mut self.inner
    }

    /// Access the outbox aggregate (read-only).
    pub fn outbox(&self) -> impl Deref<Target = Outbox> + '_ {
        OutboxRef(self.outbox.read().unwrap())
    }

    /// Access the outbox aggregate (mutable).
    pub fn outbox_mut(&self) -> impl DerefMut<Target = Outbox> + '_ {
        OutboxRefMut(self.outbox.write().unwrap())
    }
}

impl<R: Repository> OutboxRepository<R> {
    /// Commit only the outbox state changes.
    ///
    /// Use this after `process_next` to persist each message's status
    /// immediately, ensuring crash-safe at-least-once delivery.
    pub fn commit_outbox(&self) -> Result<(), RepositoryError> {
        let outbox = self.outbox.read().map_err(|_| RepositoryError::LockPoisoned("outbox read"))?;

        // Create a temporary entity with the outbox's events for persistence
        let mut outbox_entity = Entity::with_id(outbox.entity.id());
        outbox_entity.load_from_history(outbox.entity.events().to_vec());

        // Commit the outbox entity to the inner repository
        self.inner.commit(&mut outbox_entity)
    }
}

// Wrapper types for Deref/DerefMut implementation
struct OutboxRef<'a>(RwLockReadGuard<'a, Outbox>);
struct OutboxRefMut<'a>(RwLockWriteGuard<'a, Outbox>);

impl Deref for OutboxRef<'_> {
    type Target = Outbox;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Deref for OutboxRefMut<'_> {
    type Target = Outbox;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for OutboxRefMut<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<R: Repository> Repository for OutboxRepository<R> {
    fn get(&self, id: &str) -> Result<Option<Entity>, RepositoryError> {
        self.inner.get(id)
    }

    fn get_all(&self, ids: &[&str]) -> Result<Vec<Entity>, RepositoryError> {
        self.inner.get_all(ids)
    }

    fn commit<C: Committable + ?Sized>(&self, committable: &mut C) -> Result<(), RepositoryError> {
        // First, commit the entities to the inner repository
        // (this also collects and clears outbox events via take_outbox_events)
        let outbox_events = committable.take_outbox_events();

        // Commit entities
        self.inner.commit(committable)?;

        // Then add outbox events to our Outbox aggregate and persist it
        if !outbox_events.is_empty() {
            let mut outbox = self.outbox.write().map_err(|_| RepositoryError::LockPoisoned("outbox write"))?;

            for (event_type, payload) in outbox_events {
                outbox.put(&event_type, &payload);
            }

            // Persist the outbox aggregate
            // Create a temporary entity to commit
            let mut outbox_entity = Entity::with_id(outbox.entity.id());
            outbox_entity.load_from_history(outbox.entity.events().to_vec());

            drop(outbox); // Release lock before committing
            self.inner.commit(&mut outbox_entity)?;
        }

        Ok(())
    }
}

/// Extension trait for wrapping a repository with outbox capability.
pub trait WithOutbox: Repository + Sized {
    /// Wrap this repository with outbox capability.
    fn with_outbox(self) -> OutboxRepository<Self> {
        OutboxRepository::new(self)
    }

    /// Wrap this repository with outbox capability and custom outbox ID.
    fn with_outbox_id(self, id: impl Into<String>) -> OutboxRepository<Self> {
        OutboxRepository::with_outbox_id(self, id)
    }
}

impl<R: Repository> WithOutbox for R {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HashMapRepository;
    use crate::outbox::OutboxEntity;

    #[test]
    fn new_outbox_repository() {
        let storage = HashMapRepository::new();
        let repo = OutboxRepository::new(storage);

        assert_eq!(repo.outbox().pending_count(), 0);
    }

    #[test]
    fn with_outbox_extension() {
        let repo = HashMapRepository::new().with_outbox();

        assert_eq!(repo.outbox().pending_count(), 0);
    }

    #[test]
    fn commit_entity_with_outbox_events() {
        let repo = HashMapRepository::new().with_outbox();

        let mut entity = OutboxEntity::with_id("test");
        entity.digest("Created", vec!["test".to_string()]);
        entity.outbox("EntityCreated", r#"{"id":"test"}"#);

        repo.commit(&mut entity).unwrap();

        // Check entity was persisted
        let fetched = repo.get("test").unwrap().unwrap();
        assert_eq!(fetched.events().len(), 1);

        // Check outbox has the event
        let outbox = repo.outbox();
        assert_eq!(outbox.pending_count(), 1);
        assert_eq!(outbox.pending()[0].event_type, "EntityCreated");
    }

    #[test]
    fn commit_without_outbox_events() {
        let repo = HashMapRepository::new().with_outbox();

        let mut entity = Entity::with_id("test");
        entity.digest("Created", vec!["test".to_string()]);
        // No outbox events

        repo.commit(&mut entity).unwrap();

        // Check entity was persisted
        let fetched = repo.get("test").unwrap().unwrap();
        assert_eq!(fetched.events().len(), 1);

        // Outbox should be empty
        let outbox = repo.outbox();
        assert_eq!(outbox.pending_count(), 0);
    }

    #[test]
    fn inner_access() {
        let storage = HashMapRepository::new();
        let repo = OutboxRepository::new(storage);

        // Can access inner repository
        let _ = repo.inner();
    }
}
