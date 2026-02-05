use event_emitter_rs::EventEmitter;

use crate::entity::{Entity, LocalEvent};

/// Extension wrapper that adds event emitter capabilities to an Entity.
///
/// Provides `enqueue`, `on`, and `emit` functionality for running
/// callbacks after successful commits.
pub struct EntityEmitter {
    entity: Entity,
    event_emitter: EventEmitter,
    events_to_emit: Vec<LocalEvent>,
}

impl Default for EntityEmitter {
    fn default() -> Self {
        Self::new(Entity::new())
    }
}

impl EntityEmitter {
    /// Wrap an entity with emitter capabilities.
    pub fn new(entity: Entity) -> Self {
        Self {
            entity,
            event_emitter: EventEmitter::new(),
            events_to_emit: Vec::new(),
        }
    }

    /// Get a reference to the underlying entity.
    pub fn entity(&self) -> &Entity {
        &self.entity
    }

    /// Get a mutable reference to the underlying entity.
    pub fn entity_mut(&mut self) -> &mut Entity {
        &mut self.entity
    }

    /// Unwrap and return the underlying entity.
    pub fn into_entity(self) -> Entity {
        self.entity
    }

    /// Queue an event to be emitted after commit.
    pub fn enqueue(&mut self, event_type: impl Into<String>, data: impl Into<String>) {
        if self.entity.is_replaying() {
            return;
        }

        self.events_to_emit.push(LocalEvent {
            event_type: event_type.into(),
            data: data.into(),
        });
    }

    /// Queue an event with a serializable payload.
    /// The payload is serialized to JSON.
    pub fn enqueue_with<T: serde::Serialize>(
        &mut self,
        event_type: impl Into<String>,
        payload: &T,
    ) {
        if self.entity.is_replaying() {
            return;
        }
        let data = serde_json::to_string(payload).unwrap_or_default();
        self.events_to_emit.push(LocalEvent {
            event_type: event_type.into(),
            data,
        });
    }

    /// Drain all queued events for external emission.
    pub fn drain_queued_events(&mut self) -> Vec<LocalEvent> {
        self.events_to_emit.drain(..).collect()
    }

    /// Check if the underlying entity is replaying.
    pub fn is_replaying(&self) -> bool {
        self.entity.is_replaying()
    }

    /// Set the replaying flag on the underlying entity.
    pub fn set_replaying(&mut self, replaying: bool) {
        self.entity.set_replaying(replaying);
    }

    /// Register a listener for an event type.
    pub fn on<F>(&mut self, event: &str, listener: F)
    where
        F: Fn(String) + Send + Sync + 'static,
    {
        self.event_emitter.on(event, listener);
    }

    /// Emit an event immediately.
    pub fn emit(&mut self, event: &str, data: impl Into<String>) {
        self.event_emitter.emit(event, data.into());
    }

    /// Emit all queued events. Call this after a successful commit.
    pub fn emit_queued(&mut self) {
        let events: Vec<_> = self.events_to_emit.drain(..).collect();
        for event in events {
            self.emit(&event.event_type, event.data);
        }
    }

    /// Number of events queued for emission.
    pub fn queued_len(&self) -> usize {
        self.events_to_emit.len()
    }
}

/// Trait for types that can be extended with emitter capabilities.
pub trait EmittableEntity {
    /// Wrap with emitter capabilities.
    fn with_emitter(self) -> EntityEmitter;
}

impl EmittableEntity for Entity {
    fn with_emitter(self) -> EntityEmitter {
        EntityEmitter::new(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn enqueue_and_emit() {
        let entity = Entity::with_id("test");
        let mut emitter = entity.with_emitter();

        let called = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&called);

        emitter.on("TestEvent", move |data| {
            assert_eq!(data, "test payload");
            flag.store(true, Ordering::SeqCst);
        });

        emitter.enqueue("TestEvent", "test payload");
        assert_eq!(emitter.queued_len(), 1);

        emitter.emit_queued();
        assert_eq!(emitter.queued_len(), 0);

        // EventEmitter is async, give it time
        thread::sleep(Duration::from_millis(50));
        assert!(called.load(Ordering::SeqCst));
    }

    #[test]
    fn entity_access() {
        let entity = Entity::with_id("test");
        let mut emitter = entity.with_emitter();

        assert_eq!(emitter.entity().id(), "test");

        emitter.entity_mut().set_id("changed");
        assert_eq!(emitter.entity().id(), "changed");

        let entity = emitter.into_entity();
        assert_eq!(entity.id(), "changed");
    }
}
