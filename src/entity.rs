use std::fmt;
use std::time::SystemTime;

use event_emitter_rs::EventEmitter;
use serde::{Deserialize, Serialize};

use crate::event_record::EventRecord;
use crate::local_event::LocalEvent;

fn default_event_emitter() -> EventEmitter {
    EventEmitter::new()
}

#[derive(Clone, Debug)]
pub(crate) struct OutboxEvent {
    pub(crate) event_type: String,
    pub(crate) payload: String,
}

#[derive(Serialize, Deserialize)]
pub struct Entity {
    id: String,
    version: u64,
    events: Vec<EventRecord>,
    #[serde(skip, default)]
    events_to_emit: Vec<LocalEvent>,
    #[serde(skip, default)]
    outbox_events: Vec<OutboxEvent>,
    #[serde(skip, default)]
    replaying: bool,
    snapshot_version: u64,
    timestamp: SystemTime,
    #[serde(skip, default = "default_event_emitter")]
    event_emitter: EventEmitter,
}

impl Default for Entity {
    fn default() -> Self {
        Entity {
            id: String::new(),
            version: 0,
            events: Vec::new(),
            events_to_emit: Vec::new(),
            outbox_events: Vec::new(),
            replaying: false,
            snapshot_version: 0,
            timestamp: SystemTime::now(),
            event_emitter: EventEmitter::new(),
        }
    }
}

impl fmt::Debug for Entity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Entity")
            .field("id", &self.id)
            .field("version", &self.version)
            .field("events", &self.events)
            .field("events_to_emit", &self.events_to_emit)
            .field("outbox_events", &self.outbox_events)
            .field("replaying", &self.replaying)
            .field("snapshot_version", &self.snapshot_version)
            .field("timestamp", &self.timestamp)
            .finish()
    }
}

impl Clone for Entity {
    fn clone(&self) -> Self {
        Entity {
            id: self.id.clone(),
            version: self.version,
            events: self.events.clone(),
            events_to_emit: self.events_to_emit.clone(),
            outbox_events: self.outbox_events.clone(),
            replaying: self.replaying,
            snapshot_version: self.snapshot_version,
            timestamp: self.timestamp,
            event_emitter: EventEmitter::new(),
        }
    }
}

struct ReplayGuard<'a> {
    replaying: &'a mut bool,
}

impl<'a> ReplayGuard<'a> {
    fn new(replaying: &'a mut bool) -> Self {
        *replaying = true;
        ReplayGuard { replaying }
    }
}

impl Drop for ReplayGuard<'_> {
    fn drop(&mut self) {
        *self.replaying = false;
    }
}

impl Entity {
    pub fn new() -> Self {
        Entity::default()
    }

    pub fn with_id(id: impl Into<String>) -> Self {
        let mut entity = Entity::default();
        entity.id = id.into();
        entity
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn set_id(&mut self, id: impl Into<String>) {
        self.id = id.into();
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn snapshot_version(&self) -> u64 {
        self.snapshot_version
    }

    pub fn set_snapshot_version(&mut self, snapshot_version: u64) {
        self.snapshot_version = snapshot_version;
    }

    pub fn events(&self) -> &[EventRecord] {
        &self.events
    }

    pub fn queued_events_len(&self) -> usize {
        self.events_to_emit.len()
    }

    pub fn outbox_len(&self) -> usize {
        self.outbox_events.len()
    }

    pub fn digest(&mut self, name: impl Into<String>, args: Vec<String>) {
        if self.replaying {
            return;
        }

        let sequence = self.events.len() as u64 + 1;
        let record = EventRecord::new(name, args, sequence);
        self.events.push(record);
        self.version = self.events.len() as u64;
        self.timestamp = SystemTime::now();
    }

    pub fn enqueue(&mut self, event_type: impl Into<String>, data: impl Into<String>) {
        if self.replaying {
            return;
        }

        self.events_to_emit.push(LocalEvent {
            event_type: event_type.into(),
            data: data.into(),
        });
    }

    pub fn outbox(&mut self, event_type: impl Into<String>, payload: impl Into<String>) {
        if self.replaying {
            return;
        }

        // Outbox events are persisted by repositories that implement OutboxRepository,
        // ideally in the same transaction as the event stream.
        self.outbox_events.push(OutboxEvent {
            event_type: event_type.into(),
            payload: payload.into(),
        });
    }

    pub(crate) fn outbox_events(&self) -> &[OutboxEvent] {
        &self.outbox_events
    }

    pub(crate) fn clear_outbox(&mut self) {
        self.outbox_events.clear();
    }

    pub fn emit_queued_events(&mut self) {
        let events: Vec<_> = self.events_to_emit.drain(..).collect();

        for event in events {
            self.emit(&event.event_type, event.data);
        }
    }

    pub fn emit(&mut self, event: &str, data: impl Into<String>) {
        self.event_emitter.emit(event, data.into());
    }

    pub fn on<F>(&mut self, event: &str, listener: F)
    where
        F: Fn(String) + Send + Sync + 'static,
    {
        self.event_emitter.on(event, listener);
    }

    pub fn load_from_history(&mut self, history: Vec<EventRecord>) {
        self.events = history;
        self.version = self.events.len() as u64;
        self.events_to_emit.clear();
        self.outbox_events.clear();
    }

    pub fn rehydrate<F, E>(&mut self, mut apply: F) -> Result<(), E>
    where
        F: FnMut(&EventRecord) -> Result<(), E>,
    {
        let _guard = ReplayGuard::new(&mut self.replaying);

        for event in &self.events {
            apply(event)?;
        }

        Ok(())
    }

    pub fn is_replaying(&self) -> bool {
        self.replaying
    }

    pub fn set_replaying(&mut self, replaying: bool) {
        self.replaying = replaying;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    #[test]
    fn new() {
        let entity = Entity::new();
        assert_eq!(entity.id(), "");
        assert_eq!(entity.version(), 0);
        assert!(entity.events().is_empty());
        assert_eq!(entity.queued_events_len(), 0);
        assert_eq!(entity.outbox_len(), 0);
        assert!(!entity.is_replaying());
        assert_eq!(entity.snapshot_version(), 0);
    }

    #[test]
    fn digest() {
        let mut entity = Entity::new();
        entity.digest(
            "test_event",
            vec!["arg1".to_string(), "arg2".to_string()],
        );

        assert_eq!(entity.version(), 1);
        assert_eq!(entity.events().len(), 1);
        assert_eq!(entity.events()[0].event_name, "test_event");
        assert_eq!(entity.events()[0].args, vec!["arg1", "arg2"]);
        assert_eq!(entity.events()[0].sequence, 1);
    }

    #[test]
    fn enqueue() {
        let mut entity = Entity::new();
        entity.enqueue("test_event", "test_data");

        assert_eq!(entity.queued_events_len(), 1);
    }

    #[test]
    fn outbox() {
        let mut entity = Entity::new();
        entity.outbox("DomainEvent", "{\"ok\":true}");

        assert_eq!(entity.outbox_len(), 1);
        assert_eq!(entity.outbox_events()[0].event_type, "DomainEvent");
    }

    #[test]
    fn emit_queued_events() {
        let mut entity = Entity::new();
        entity.enqueue("test_event1", "test_data1");
        entity.enqueue("test_event2", "test_data2");

        assert_eq!(entity.queued_events_len(), 2);

        entity.emit_queued_events();

        assert_eq!(entity.queued_events_len(), 0);
    }

    #[test]
    fn rehydrate() {
        let mut entity = Entity::new();
        entity.digest("test_event1", vec!["arg1".to_string()]);
        entity.digest("test_event2", vec!["arg2".to_string()]);

        let mut replayed = Vec::new();
        let result = entity.rehydrate(|event| {
            replayed.push(event.event_name.clone());
            Ok::<(), ()>(())
        });

        assert!(result.is_ok());
        assert_eq!(replayed, vec!["test_event1", "test_event2"]);
        assert!(!entity.is_replaying());
    }

    #[test]
    fn clone() {
        let entity = Entity::new();
        let cloned_entity = entity.clone();

        assert_eq!(entity.id(), cloned_entity.id());
        assert_eq!(entity.version(), cloned_entity.version());
        assert_eq!(entity.events(), cloned_entity.events());
        assert_eq!(entity.snapshot_version(), cloned_entity.snapshot_version());
    }

    #[test]
    fn debug() {
        let entity = Entity::new();
        let debug_str = format!("{:?}", entity);

        assert!(debug_str.contains("Entity"));
        assert!(debug_str.contains("id: \"\""));
        assert!(debug_str.contains("version: 0"));
    }

    #[test]
    fn serialize_deserialize() {
        let mut entity = Entity::new();
        entity.digest("test_event1", vec!["arg1".to_string()]);

        let serialized: String = serde_json::to_string(&entity).unwrap();
        let deserialized: Entity = serde_json::from_str(&serialized).unwrap();

        assert_eq!(entity.id(), deserialized.id());
        assert_eq!(entity.version(), deserialized.version());
        assert_eq!(entity.events(), deserialized.events());
        assert_eq!(entity.snapshot_version(), deserialized.snapshot_version());
        assert_eq!(entity.timestamp, deserialized.timestamp);
    }

    #[test]
    fn emit_and_on() {
        let mut entity = Entity::new();
        let event_name = "test_event";
        let event_data = "test_data";

        entity.on(event_name, move |data| {
            assert_eq!(*event_data, data);
        });

        entity.emit(event_name, event_data);
    }

    #[test]
    fn replaying_state_blocks_changes() {
        let mut entity = Entity::new();
        entity.replaying = true;

        entity.digest("test_event", vec!["arg1".to_string()]);
        assert!(entity.events().is_empty());

        entity.enqueue("test_event", "test_data");
        assert_eq!(entity.queued_events_len(), 0);

        entity.outbox("DomainEvent", "{\"ok\":true}");
        assert_eq!(entity.outbox_len(), 0);
    }
}
