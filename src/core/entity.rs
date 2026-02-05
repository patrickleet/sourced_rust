use std::fmt;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use super::event_record::EventRecord;

#[derive(Serialize, Deserialize)]
pub struct Entity {
    id: String,
    version: u64,
    events: Vec<EventRecord>,
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

    /// Record an event with a serializable payload.
    /// The payload is serialized using bitcode for compact, fast storage.
    pub fn digest<T: serde::Serialize>(&mut self, name: impl Into<String>, payload: &T) {
        if self.replaying {
            return;
        }

        let bytes = bitcode::serialize(payload).expect("failed to serialize payload");
        let sequence = self.events.len() as u64 + 1;
        let record = EventRecord::new(name, bytes, sequence);
        self.events.push(record);
        self.version = self.events.len() as u64;
        self.timestamp = SystemTime::now();
    }

    /// Record an event with no payload.
    pub fn digest_empty(&mut self, name: impl Into<String>) {
        self.digest(name, &());
    }

    pub fn load_from_history(&mut self, history: Vec<EventRecord>) {
        self.events = history;
        self.version = self.events.len() as u64;
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

    /// Replace all events with a single snapshot event.
    /// Used by projections to store current state.
    pub fn set_snapshot<T: serde::Serialize>(&mut self, data: &T) {
        let payload = bitcode::serialize(data).expect("failed to serialize snapshot");
        self.events.clear();
        let record = EventRecord::new("Snapshot", payload, 1);
        self.events.push(record);
        self.version = 1;
        self.timestamp = SystemTime::now();
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
        assert!(!entity.is_replaying());
        assert_eq!(entity.snapshot_version(), 0);
    }

    #[test]
    fn digest() {
        let mut entity = Entity::new();
        entity.digest("test_event", &("arg1", "arg2"));

        assert_eq!(entity.version(), 1);
        assert_eq!(entity.events().len(), 1);
        assert_eq!(entity.events()[0].event_name, "test_event");
        let decoded: (String, String) = entity.events()[0].decode().unwrap();
        assert_eq!(decoded, ("arg1".to_string(), "arg2".to_string()));
        assert_eq!(entity.events()[0].sequence, 1);
    }

    #[test]
    fn rehydrate() {
        let mut entity = Entity::new();
        entity.digest("test_event1", &"arg1");
        entity.digest("test_event2", &"arg2");

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
        entity.digest("test_event1", &"arg1");

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

        entity.digest("test_event", &"arg1");
        assert!(entity.events().is_empty());
    }
}
