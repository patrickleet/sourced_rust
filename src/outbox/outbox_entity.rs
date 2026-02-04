use std::ops::{Deref, DerefMut};

use crate::core::{Committable, Entity};

/// Event to be published to the outbox.
#[derive(Clone, Debug)]
pub struct OutboxEvent {
    pub event_type: String,
    pub payload: String,
}

/// Entity wrapper that provides outbox capability.
///
/// This is an alternative to using `Entity::outbox()` directly.
/// It wraps an Entity and manages outbox events separately,
/// making the outbox functionality explicit and optional.
///
/// # Example
///
/// ```ignore
/// use sourced_rust::outbox::OutboxEntity;
/// use sourced_rust::Entity;
///
/// let entity = Entity::with_id("123");
/// let mut outbox_entity = OutboxEntity::new(entity);
///
/// outbox_entity.digest("Created", vec!["123".to_string()]);
/// outbox_entity.outbox("EntityCreated", r#"{"id":"123"}"#);
///
/// repo.commit(&mut outbox_entity)?;
/// ```
pub struct OutboxEntity {
    entity: Entity,
    outbox_events: Vec<OutboxEvent>,
}

impl OutboxEntity {
    /// Wrap an entity with outbox capability.
    pub fn new(entity: Entity) -> Self {
        Self {
            entity,
            outbox_events: Vec::new(),
        }
    }

    /// Create a new OutboxEntity with the given ID.
    pub fn with_id(id: impl Into<String>) -> Self {
        Self::new(Entity::with_id(id))
    }

    /// Queue an event for the outbox.
    ///
    /// These events will be atomically committed with the entity
    /// when the repository commits.
    pub fn outbox(&mut self, event_type: impl Into<String>, payload: impl Into<String>) {
        if self.entity.is_replaying() {
            return;
        }

        self.outbox_events.push(OutboxEvent {
            event_type: event_type.into(),
            payload: payload.into(),
        });
    }

    /// Get the queued outbox events.
    pub fn outbox_events(&self) -> &[OutboxEvent] {
        &self.outbox_events
    }

    /// Clear the outbox events (called after commit).
    pub fn clear_outbox(&mut self) {
        self.outbox_events.clear();
    }

    /// Number of queued outbox events.
    pub fn outbox_len(&self) -> usize {
        self.outbox_events.len()
    }

    /// Unwrap and return the underlying entity.
    pub fn into_entity(self) -> Entity {
        self.entity
    }

    /// Get a reference to the underlying entity.
    pub fn entity(&self) -> &Entity {
        &self.entity
    }

    /// Get a mutable reference to the underlying entity.
    pub fn entity_mut(&mut self) -> &mut Entity {
        &mut self.entity
    }
}

// Deref to Entity for convenience - allows calling entity methods directly
impl Deref for OutboxEntity {
    type Target = Entity;

    fn deref(&self) -> &Self::Target {
        &self.entity
    }
}

impl DerefMut for OutboxEntity {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.entity
    }
}

// Implement Committable so OutboxEntity can be committed directly
impl Committable for OutboxEntity {
    fn entities_mut(&mut self) -> Vec<&mut Entity> {
        vec![&mut self.entity]
    }

    fn take_outbox_events(&mut self) -> Vec<(String, String)> {
        // Return OutboxEntity's own outbox events, not the inner entity's
        let events: Vec<_> = self
            .outbox_events
            .drain(..)
            .map(|e| (e.event_type, e.payload))
            .collect();
        events
    }
}

/// Trait for types that can provide outbox events.
///
/// This allows repository implementations to check if a committable
/// has outbox events to process.
pub trait HasOutbox {
    /// Get the outbox events to be committed.
    fn outbox_events(&self) -> &[OutboxEvent];

    /// Clear the outbox events after commit.
    fn clear_outbox(&mut self);
}

impl HasOutbox for OutboxEntity {
    fn outbox_events(&self) -> &[OutboxEvent] {
        &self.outbox_events
    }

    fn clear_outbox(&mut self) {
        self.outbox_events.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_outbox_entity() {
        let entity = Entity::with_id("test");
        let outbox_entity = OutboxEntity::new(entity);

        assert_eq!(outbox_entity.id(), "test");
        assert_eq!(outbox_entity.outbox_len(), 0);
    }

    #[test]
    fn with_id() {
        let outbox_entity = OutboxEntity::with_id("test");
        assert_eq!(outbox_entity.id(), "test");
    }

    #[test]
    fn outbox_queues_events() {
        let mut outbox_entity = OutboxEntity::with_id("test");

        outbox_entity.outbox("Event1", "payload1");
        outbox_entity.outbox("Event2", "payload2");

        assert_eq!(outbox_entity.outbox_len(), 2);
        assert_eq!(outbox_entity.outbox_events()[0].event_type, "Event1");
        assert_eq!(outbox_entity.outbox_events()[1].event_type, "Event2");
    }

    #[test]
    fn clear_outbox() {
        let mut outbox_entity = OutboxEntity::with_id("test");
        outbox_entity.outbox("Event1", "payload1");

        assert_eq!(outbox_entity.outbox_len(), 1);

        outbox_entity.clear_outbox();
        assert_eq!(outbox_entity.outbox_len(), 0);
    }

    #[test]
    fn deref_to_entity() {
        let mut outbox_entity = OutboxEntity::with_id("test");

        // Can call Entity methods directly via Deref
        outbox_entity.digest("Created", vec!["test".to_string()]);

        assert_eq!(outbox_entity.version(), 1);
        assert_eq!(outbox_entity.events().len(), 1);
    }

    #[test]
    fn replaying_blocks_outbox() {
        let mut entity = Entity::with_id("test");
        entity.set_replaying(true);

        let mut outbox_entity = OutboxEntity::new(entity);
        outbox_entity.outbox("Event1", "payload1");

        assert_eq!(outbox_entity.outbox_len(), 0);
    }

    #[test]
    fn committable_returns_entity() {
        let mut outbox_entity = OutboxEntity::with_id("test");

        let entities = outbox_entity.entities_mut();
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].id(), "test");
    }
}
