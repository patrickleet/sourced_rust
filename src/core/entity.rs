use std::fmt;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use super::event_record::EventRecord;

#[derive(Clone, Debug)]
pub struct OutboxEvent {
    pub event_type: String,
    pub payload: String,
}

#[derive(Serialize, Deserialize)]
pub struct Entity {
    id: String,
    version: u64,
    events: Vec<EventRecord>,
    #[serde(skip, default)]
    outbox_events: Vec<OutboxEvent>,
    #[serde(skip, default)]
    replaying: bool,
    snapshot_version: u64,
    timestamp: SystemTime,
}

impl Default for Entity {
    fn default() -> Self {
        Entity {
            id: String::new(),
            version: 0,
            events: Vec::new(),
            outbox_events: Vec::new(),
            replaying: false,
            snapshot_version: 0,
            timestamp: SystemTime::now(),
        }
    }
}

impl fmt::Debug for Entity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Entity")
            .field("id", &self.id)
            .field("version", &self.version)
            .field("events", &self.events)
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
            outbox_events: self.outbox_events.clone(),
            replaying: self.replaying,
            snapshot_version: self.snapshot_version,
            timestamp: self.timestamp,
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

    pub fn outbox(&mut self, event_type: impl Into<String>, payload: impl Into<String>) {
        if self.replaying {
            return;
        }

        self.outbox_events.push(OutboxEvent {
            event_type: event_type.into(),
            payload: payload.into(),
        });
    }

    pub fn outbox_events(&self) -> &[OutboxEvent] {
        &self.outbox_events
    }

    pub fn clear_outbox(&mut self) {
        self.outbox_events.clear();
    }

    pub fn load_from_history(&mut self, history: Vec<EventRecord>) {
        self.events = history;
        self.version = self.events.len() as u64;
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
    fn outbox() {
        let mut entity = Entity::new();
        entity.outbox("DomainEvent", "{\"ok\":true}");

        assert_eq!(entity.outbox_len(), 1);
        assert_eq!(entity.outbox_events()[0].event_type, "DomainEvent");
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
    fn replaying_state_blocks_changes() {
        let mut entity = Entity::new();
        entity.replaying = true;

        entity.digest("test_event", vec!["arg1".to_string()]);
        assert!(entity.events().is_empty());

        entity.outbox("DomainEvent", "{\"ok\":true}");
        assert_eq!(entity.outbox_len(), 0);
    }
}
