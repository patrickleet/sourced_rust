mod aggregate;

use aggregate::{TodoV1, TodoV2, TodoV3};
use sourced_rust::{
    hydrate, Aggregate, AggregateBuilder, Commit, Entity, EventRecord, EventUpcaster,
    HashMapRepository, SnapshotStore, upcast_events,
};

// =============================================================================
// EventRecord version field
// =============================================================================

#[test]
fn event_record_defaults_to_version_1() {
    let record = EventRecord::new("TestEvent", vec![], 1);
    assert_eq!(record.event_version, 1);
}

#[test]
fn event_record_new_versioned() {
    let record = EventRecord::new_versioned("TestEvent", vec![], 1, 3);
    assert_eq!(record.event_version, 3);
    assert_eq!(record.event_name, "TestEvent");
}

#[test]
fn event_version_serializes_cleanly_when_v1() {
    let record = EventRecord::new("TestEvent", vec![], 1);
    let json = serde_json::to_string(&record).unwrap();
    // Version 1 is skipped in serialization
    assert!(!json.contains("event_version"));
}

#[test]
fn event_version_serializes_when_not_v1() {
    let record = EventRecord::new_versioned("TestEvent", vec![], 1, 2);
    let json = serde_json::to_string(&record).unwrap();
    assert!(json.contains("\"event_version\":2"));
}

#[test]
fn old_events_without_event_version_deserialize_as_v1() {
    let json = r#"{"event_name":"old_event","payload":"","sequence":1,"timestamp":{"secs_since_epoch":0,"nanos_since_epoch":0}}"#;
    let record: EventRecord = serde_json::from_str(json).unwrap();
    assert_eq!(record.event_version, 1);
}

#[test]
fn event_version_round_trips_through_serde() {
    let record = EventRecord::new_versioned("TestEvent", vec![1, 2, 3], 1, 5);
    let json = serde_json::to_string(&record).unwrap();
    let deserialized: EventRecord = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.event_version, 5);
    assert_eq!(deserialized.event_name, "TestEvent");
    assert_eq!(deserialized.sequence, 1);
}

// =============================================================================
// digest version = N macro
// =============================================================================

#[test]
fn digest_v_creates_events_at_specified_version() {
    let mut todo = TodoV2::default();
    todo.initialize("t1".into(), "alice".into(), "Buy milk".into(), 3);

    let events = todo.entity.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_version, 2);
    assert_eq!(events[0].event_name, "Initialized");
}

#[test]
fn digest_without_version_creates_v1_events() {
    let mut todo = TodoV1::default();
    todo.initialize("t1".into(), "alice".into(), "Buy milk".into());

    let events = todo.entity.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_version, 1);
}

// =============================================================================
// Upcaster: single v1 → v2
// =============================================================================

#[test]
fn hydrate_v2_from_v1_events() {
    // Create events using the v1 aggregate
    let mut v1 = TodoV1::default();
    v1.initialize("t1".into(), "alice".into(), "Buy milk".into());
    v1.complete();

    // Simulate loading into the v2 aggregate by transferring the entity
    let mut entity = Entity::new();
    entity.load_from_history(v1.entity.events().to_vec());

    let v2: TodoV2 = hydrate(entity).unwrap();
    assert_eq!(v2.entity.id(), "t1");
    assert_eq!(v2.user_id, "alice");
    assert_eq!(v2.task, "Buy milk");
    assert_eq!(v2.priority, 0); // default from upcaster
    assert!(v2.completed);
}

#[test]
fn v2_aggregate_has_upcasters() {
    let upcasters = TodoV2::upcasters();
    assert_eq!(upcasters.len(), 1);
    assert_eq!(upcasters[0].event_type, "Initialized");
    assert_eq!(upcasters[0].from_version, 1);
    assert_eq!(upcasters[0].to_version, 2);
}

#[test]
fn v1_aggregate_has_no_upcasters() {
    let upcasters = TodoV1::upcasters();
    assert!(upcasters.is_empty());
}

// =============================================================================
// Chained upcasters: v1 → v2 → v3
// =============================================================================

#[test]
fn hydrate_v3_from_v1_events_chains_upcasters() {
    // Create events using v1 aggregate
    let mut v1 = TodoV1::default();
    v1.initialize("t1".into(), "bob".into(), "Walk dog".into());

    let mut entity = Entity::new();
    entity.load_from_history(v1.entity.events().to_vec());

    let v3: TodoV3 = hydrate(entity).unwrap();
    assert_eq!(v3.entity.id(), "t1");
    assert_eq!(v3.user_id, "bob");
    assert_eq!(v3.task, "Walk dog");
    assert_eq!(v3.priority, 0);     // from v1→v2 upcaster
    assert_eq!(v3.due_date, "");    // from v2→v3 upcaster
    assert!(!v3.completed);
}

#[test]
fn hydrate_v3_from_v2_events_applies_single_upcaster() {
    // Create events using v2 aggregate
    let mut v2 = TodoV2::default();
    v2.initialize("t1".into(), "carol".into(), "Read book".into(), 5);

    let mut entity = Entity::new();
    entity.load_from_history(v2.entity.events().to_vec());

    let v3: TodoV3 = hydrate(entity).unwrap();
    assert_eq!(v3.entity.id(), "t1");
    assert_eq!(v3.user_id, "carol");
    assert_eq!(v3.task, "Read book");
    assert_eq!(v3.priority, 5);     // preserved from v2
    assert_eq!(v3.due_date, "");    // from v2→v3 upcaster
}

#[test]
fn hydrate_v3_from_native_v3_events_no_upcasting_needed() {
    let mut v3 = TodoV3::default();
    v3.initialize("t1".into(), "dave".into(), "Cook dinner".into(), 2, "2025-12-31".into());

    let mut entity = Entity::new();
    entity.load_from_history(v3.entity.events().to_vec());

    let loaded: TodoV3 = hydrate(entity).unwrap();
    assert_eq!(loaded.priority, 2);
    assert_eq!(loaded.due_date, "2025-12-31");
}

#[test]
fn v3_aggregate_has_two_upcasters() {
    let upcasters = TodoV3::upcasters();
    assert_eq!(upcasters.len(), 2);
    assert_eq!(upcasters[0].from_version, 1);
    assert_eq!(upcasters[0].to_version, 2);
    assert_eq!(upcasters[1].from_version, 2);
    assert_eq!(upcasters[1].to_version, 3);
}

// =============================================================================
// Mixed events: some need upcasting, some don't
// =============================================================================

#[test]
fn mixed_events_v1_init_and_v1_complete() {
    // Completed events are always v1 — no upcaster needed
    let mut v1 = TodoV1::default();
    v1.initialize("t1".into(), "eve".into(), "Test".into());
    v1.complete();

    let mut entity = Entity::new();
    entity.load_from_history(v1.entity.events().to_vec());

    let v2: TodoV2 = hydrate(entity).unwrap();
    assert_eq!(v2.priority, 0);
    assert!(v2.completed);
}

// =============================================================================
// Repository round-trip with upcasting
// =============================================================================

#[test]
fn repo_roundtrip_v1_to_v2() {
    // Store using v1
    let v1_repo = HashMapRepository::new();
    let mut v1 = TodoV1::default();
    v1.initialize("t1".into(), "frank".into(), "Shop".into());
    v1_repo.commit(&mut v1.entity).unwrap();

    // Load using v2 (same storage)
    let v2_repo = v1_repo.aggregate::<TodoV2>();
    let loaded = v2_repo.get("t1").unwrap().unwrap();
    assert_eq!(loaded.user_id, "frank");
    assert_eq!(loaded.task, "Shop");
    assert_eq!(loaded.priority, 0);
}

// =============================================================================
// upcast_events standalone function
// =============================================================================

#[test]
fn upcast_events_standalone() {
    let payload_v1 = bitcode::serialize(&("id1".to_string(), "user1".to_string(), "task1".to_string())).unwrap();
    let event = EventRecord::new("Initialized", payload_v1, 1);

    let upcasters: &[EventUpcaster] = &[EventUpcaster {
        event_type: "Initialized",
        from_version: 1,
        to_version: 2,
        transform: aggregate::upcast_initialized_v1_v2,
    }];

    let result = upcast_events(vec![event], upcasters);
    assert_eq!(result[0].event_version, 2);

    let (id, user, task, priority): (String, String, String, u8) =
        bitcode::deserialize(&result[0].payload).unwrap();
    assert_eq!(id, "id1");
    assert_eq!(user, "user1");
    assert_eq!(task, "task1");
    assert_eq!(priority, 0);
}

// =============================================================================
// Snapshot + upcasting
// =============================================================================

#[test]
fn snapshot_plus_upcasting_post_snapshot_events() {
    // TodoV2 implements Snapshottable in aggregate.rs
    let repo = HashMapRepository::new()
        .aggregate::<TodoV2>()
        .with_snapshots(1);

    // Create using native v2 with a specific priority
    let mut todo = TodoV2::default();
    todo.initialize("t1".into(), "grace".into(), "Run".into(), 7);
    repo.commit(&mut todo).unwrap();

    // Snapshot should now exist at version 1
    assert!(repo.repo().repo().get_snapshot("t1").unwrap().is_some());

    // Add another event; this triggers snapshot + partial replay path
    let mut todo = repo.get("t1").unwrap().unwrap();
    todo.complete();
    repo.commit(&mut todo).unwrap();

    let loaded = repo.get("t1").unwrap().unwrap();
    assert_eq!(loaded.priority, 7);
    assert!(loaded.completed);
}

#[test]
fn snapshot_repo_with_v1_events_upcasted_on_hydrate() {
    // Store v1 events, then load with v2 snapshot repo
    let base_repo = HashMapRepository::new();

    // Create a v1 todo
    let mut v1 = TodoV1::default();
    v1.initialize("t1".into(), "hank".into(), "Sweep".into());
    v1.complete();
    base_repo.commit(&mut v1.entity).unwrap();

    // Load via a v2 snapshot-aware repo (no snapshot exists, so full replay with upcasting)
    let repo = base_repo
        .aggregate::<TodoV2>()
        .with_snapshots(5);

    let loaded = repo.get("t1").unwrap().unwrap();
    assert_eq!(loaded.user_id, "hank");
    assert_eq!(loaded.task, "Sweep");
    assert_eq!(loaded.priority, 0); // upcasted default
    assert!(loaded.completed);
}
