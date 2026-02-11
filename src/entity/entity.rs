use std::collections::HashMap;
use std::fmt;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use super::EventRecord;

#[derive(Serialize, Deserialize)]
pub struct Entity {
    id: String,
    version: u64,
    events: Vec<EventRecord>,
    #[serde(skip, default)]
    replaying: bool,
    snapshot_version: u64,
    #[serde(skip, default)]
    committed_version: u64,
    timestamp: SystemTime,
    /// Metadata applied to every event produced by `digest`.
    /// Transient — not serialized with the entity. Set before calling
    /// command methods to attach correlation IDs, user context, etc.
    #[serde(skip, default)]
    metadata: HashMap<String, String>,
}

impl Default for Entity {
    fn default() -> Self {
        Entity {
            id: String::new(),
            version: 0,
            events: Vec::new(),
            replaying: false,
            snapshot_version: 0,
            committed_version: 0,
            timestamp: SystemTime::now(),
            metadata: HashMap::new(),
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
            .field("committed_version", &self.committed_version)
            .field("timestamp", &self.timestamp)
            .field("metadata", &self.metadata)
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
            committed_version: self.committed_version,
            timestamp: self.timestamp,
            metadata: self.metadata.clone(),
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

    pub fn committed_version(&self) -> u64 {
        self.committed_version
    }

    pub fn events(&self) -> &[EventRecord] {
        &self.events
    }

    /// Returns events added since the entity was loaded (not yet persisted).
    pub fn new_events(&self) -> &[EventRecord] {
        &self.events[self.committed_version as usize..]
    }

    /// Mark all current events as committed. Called by repository after successful commit.
    pub fn mark_committed(&mut self) {
        self.committed_version = self.version;
    }

    /// Set metadata that will be attached to every subsequent event.
    ///
    /// Call this before invoking command methods to propagate context
    /// (correlation IDs, user info, trace spans) into the event stream.
    pub fn set_metadata(&mut self, metadata: HashMap<String, String>) {
        self.metadata = metadata;
    }

    /// Set a single metadata key-value pair.
    pub fn set_meta(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.metadata.insert(key.into(), value.into());
    }

    /// Set the correlation ID for subsequent events.
    pub fn set_correlation_id(&mut self, id: impl Into<String>) {
        self.set_meta("correlation_id", id);
    }

    /// Set the causation ID for subsequent events.
    pub fn set_causation_id(&mut self, id: impl Into<String>) {
        self.set_meta("causation_id", id);
    }

    /// Get the current metadata context.
    pub fn metadata(&self) -> &HashMap<String, String> {
        &self.metadata
    }

    /// Clear all metadata.
    pub fn clear_metadata(&mut self) {
        self.metadata.clear();
    }

    /// Record an event with a serializable payload.
    /// The payload is serialized using bitcode for compact, fast storage.
    /// Any metadata set on the entity is attached to the event.
    pub fn digest<T: serde::Serialize>(&mut self, name: impl Into<String>, payload: &T) {
        if self.replaying {
            return;
        }

        let bytes = bitcode::serialize(payload).expect("failed to serialize payload");
        let sequence = self.events.len() as u64 + 1;
        let mut record = EventRecord::new(name, bytes, sequence);
        if !self.metadata.is_empty() {
            record.metadata = self.metadata.clone();
        }
        self.events.push(record);
        self.version = self.events.len() as u64;
        self.timestamp = SystemTime::now();
    }

    /// Record a versioned event.
    pub fn digest_v<T: serde::Serialize>(&mut self, name: impl Into<String>, version: u64, payload: &T) {
        if self.replaying {
            return;
        }

        let bytes = bitcode::serialize(payload).expect("failed to serialize payload");
        let sequence = self.events.len() as u64 + 1;
        let mut record = EventRecord::new_versioned(name, bytes, sequence, version);
        if !self.metadata.is_empty() {
            record.metadata = self.metadata.clone();
        }
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
        self.committed_version = self.version;
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
    /// Used by read models to store current state.
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
        assert_eq!(entity.committed_version(), 0);
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
        assert_eq!(
            entity.committed_version(),
            cloned_entity.committed_version()
        );
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
        // committed_version is serde(skip) — not serialized, defaults to 0
        assert_eq!(deserialized.committed_version(), 0);
    }

    #[test]
    fn replaying_state_blocks_changes() {
        let mut entity = Entity::new();
        entity.replaying = true;

        entity.digest("test_event", &"arg1");
        assert!(entity.events().is_empty());
    }

    #[test]
    fn load_from_history_sets_committed_version() {
        let mut entity = Entity::new();
        assert_eq!(entity.committed_version(), 0);

        let mut source = Entity::new();
        source.digest("e1", &"a");
        source.digest("e2", &"b");

        entity.load_from_history(source.events().to_vec());
        assert_eq!(entity.version(), 2);
        assert_eq!(entity.committed_version(), 2);
        assert_eq!(entity.snapshot_version(), 0);
    }

    #[test]
    fn new_events_on_fresh_entity() {
        let mut entity = Entity::new();
        assert!(entity.new_events().is_empty());

        entity.digest("e1", &"a");
        entity.digest("e2", &"b");
        assert_eq!(entity.new_events().len(), 2);
        assert_eq!(entity.new_events()[0].event_name, "e1");
        assert_eq!(entity.new_events()[1].event_name, "e2");
    }

    #[test]
    fn new_events_after_load_and_digest() {
        let mut source = Entity::new();
        source.digest("e1", &"a");
        source.digest("e2", &"b");

        let mut entity = Entity::new();
        entity.load_from_history(source.events().to_vec());
        assert!(entity.new_events().is_empty());

        entity.digest("e3", &"c");
        assert_eq!(entity.new_events().len(), 1);
        assert_eq!(entity.new_events()[0].event_name, "e3");
    }

    #[test]
    fn mark_committed_resets_new_events() {
        let mut entity = Entity::new();
        entity.digest("e1", &"a");
        entity.digest("e2", &"b");
        assert_eq!(entity.new_events().len(), 2);

        entity.mark_committed();
        assert!(entity.new_events().is_empty());
        assert_eq!(entity.committed_version(), 2);
        assert_eq!(entity.snapshot_version(), 0);
        assert_eq!(entity.version(), 2);
        assert_eq!(entity.events().len(), 2);
    }

    #[test]
    fn digest_propagates_metadata_to_event_record() {
        let mut entity = Entity::new();
        entity.set_correlation_id("req-abc");
        entity.set_causation_id("cmd-xyz");
        entity.set_meta("user_id", "u-42");

        entity.digest("e1", &"payload");

        let record = &entity.events()[0];
        assert_eq!(record.correlation_id(), Some("req-abc"));
        assert_eq!(record.causation_id(), Some("cmd-xyz"));
        assert_eq!(record.meta("user_id"), Some("u-42"));
    }

    #[test]
    fn digest_without_metadata_leaves_event_record_empty() {
        let mut entity = Entity::new();
        entity.digest("e1", &"payload");

        let record = &entity.events()[0];
        assert!(record.metadata.is_empty());
        assert_eq!(record.correlation_id(), None);
    }

    #[test]
    fn metadata_is_transient_not_serialized() {
        let mut entity = Entity::new();
        entity.set_correlation_id("req-abc");
        entity.digest("e1", &"payload");

        let serialized = serde_json::to_string(&entity).unwrap();
        let deserialized: Entity = serde_json::from_str(&serialized).unwrap();

        // Entity metadata is transient (serde skip), should be empty after round-trip
        assert!(deserialized.metadata().is_empty());
        // But the event record metadata was persisted
        assert_eq!(deserialized.events()[0].correlation_id(), Some("req-abc"));
    }

    #[test]
    fn clear_metadata_stops_propagation() {
        let mut entity = Entity::new();
        entity.set_correlation_id("req-abc");
        entity.digest("e1", &"first");

        entity.clear_metadata();
        entity.digest("e2", &"second");

        assert_eq!(entity.events()[0].correlation_id(), Some("req-abc"));
        assert!(entity.events()[1].metadata.is_empty());
    }

    #[test]
    fn snapshot_version_not_affected_by_load_or_commit() {
        let mut entity = Entity::new();
        assert_eq!(entity.snapshot_version(), 0);

        // digest doesn't touch snapshot_version
        entity.digest("e1", &"a");
        entity.digest("e2", &"b");
        assert_eq!(entity.snapshot_version(), 0);

        // mark_committed doesn't touch snapshot_version
        entity.mark_committed();
        assert_eq!(entity.snapshot_version(), 0);

        // load_from_history doesn't touch snapshot_version
        let mut entity2 = Entity::new();
        entity2.load_from_history(entity.events().to_vec());
        assert_eq!(entity2.snapshot_version(), 0);
        assert_eq!(entity2.committed_version(), 2);
    }
}
