use sourced_rust::{Commit, Entity, GetOne, HashMapRepository};

// --- Event Accumulation ---

#[test]
fn digest_adds_events_with_correct_sequences() {
    let mut entity = Entity::with_id("e1");
    entity.digest("Created", &"data1");
    entity.digest("Updated", &"data2");
    entity.digest("Updated", &"data3");

    assert_eq!(entity.events().len(), 3);
    assert_eq!(entity.events()[0].sequence, 1);
    assert_eq!(entity.events()[1].sequence, 2);
    assert_eq!(entity.events()[2].sequence, 3);
}

#[test]
fn multiple_load_modify_commit_cycles_accumulate_all_events() {
    let repo = HashMapRepository::new();

    // Cycle 1: create and commit
    let mut entity = Entity::with_id("e1");
    entity.digest("Created", &"v1");
    repo.commit(&mut entity).unwrap();

    // Cycle 2: load, modify, commit
    let mut entity = repo.get_one("e1").unwrap().unwrap();
    assert_eq!(entity.events().len(), 1);
    entity.digest("Updated", &"v2");
    repo.commit(&mut entity).unwrap();

    // Cycle 3: load, modify, commit
    let mut entity = repo.get_one("e1").unwrap().unwrap();
    assert_eq!(entity.events().len(), 2);
    entity.digest("Updated", &"v3");
    repo.commit(&mut entity).unwrap();

    // Verify all events accumulated
    let entity = repo.get_one("e1").unwrap().unwrap();
    assert_eq!(entity.events().len(), 3);
    assert_eq!(entity.events()[0].event_name, "Created");
    assert_eq!(entity.events()[1].event_name, "Updated");
    assert_eq!(entity.events()[2].event_name, "Updated");
    assert_eq!(entity.version(), 3);
}

// --- Append Semantics ---

#[test]
fn commit_appends_only_new_events() {
    let repo = HashMapRepository::new();

    let mut entity = Entity::with_id("e1");
    entity.digest("Created", &"v1");
    repo.commit(&mut entity).unwrap();

    // Reload and add one more event
    let mut entity = repo.get_one("e1").unwrap().unwrap();
    entity.digest("Updated", &"v2");
    repo.commit(&mut entity).unwrap();

    // Verify via get_one: exactly 2 events
    let loaded = repo.get_one("e1").unwrap().unwrap();
    assert_eq!(loaded.events().len(), 2);
    assert_eq!(loaded.events()[0].event_name, "Created");
    assert_eq!(loaded.events()[1].event_name, "Updated");
}

#[test]
fn empty_commit_is_idempotent() {
    let repo = HashMapRepository::new();

    // Create initial state
    let mut entity = Entity::with_id("e1");
    entity.digest("Created", &"v1");
    repo.commit(&mut entity).unwrap();

    // Load and commit without changes
    let mut entity = repo.get_one("e1").unwrap().unwrap();
    assert!(entity.new_events().is_empty());
    repo.commit(&mut entity).unwrap();

    // Storage unchanged
    let loaded = repo.get_one("e1").unwrap().unwrap();
    assert_eq!(loaded.events().len(), 1);
}

#[test]
fn events_grow_monotonically() {
    let repo = HashMapRepository::new();

    for i in 0..5 {
        let mut entity = if i == 0 {
            Entity::with_id("e1")
        } else {
            repo.get_one("e1").unwrap().unwrap()
        };
        entity.digest("Event", &format!("v{}", i));
        repo.commit(&mut entity).unwrap();

        let loaded = repo.get_one("e1").unwrap().unwrap();
        assert_eq!(loaded.events().len(), i + 1);
    }
}

// --- Optimistic Concurrency ---

#[test]
fn concurrent_writes_detected() {
    let repo = HashMapRepository::new();

    // Create initial state
    let mut entity = Entity::with_id("e1");
    entity.digest("Created", &"v1");
    repo.commit(&mut entity).unwrap();

    // Two readers load the same version
    let mut reader1 = repo.get_one("e1").unwrap().unwrap();
    let mut reader2 = repo.get_one("e1").unwrap().unwrap();

    assert_eq!(reader1.committed_version(), 1);
    assert_eq!(reader2.committed_version(), 1);

    // Both modify
    reader1.digest("UpdatedByR1", &"r1");
    reader2.digest("UpdatedByR2", &"r2");

    // First commit succeeds
    repo.commit(&mut reader1).unwrap();

    // Second commit fails with ConcurrentWrite
    let err = repo.commit(&mut reader2).unwrap_err();
    match err {
        sourced_rust::RepositoryError::ConcurrentWrite {
            id,
            expected,
            actual,
        } => {
            assert_eq!(id, "e1");
            assert_eq!(expected, 1); // reader2 loaded at version 1
            assert_eq!(actual, 2); // storage now has 2 events
        }
        other => panic!("expected ConcurrentWrite, got: {:?}", other),
    }
}

#[test]
fn partial_conflict_rolls_back_entire_commit() {
    let repo = HashMapRepository::new();

    // Create two entities
    let mut e1 = Entity::with_id("e1");
    e1.digest("Created", &"v1");
    let mut e2 = Entity::with_id("e2");
    e2.digest("Created", &"v1");
    repo.commit(&mut [&mut e1, &mut e2]).unwrap();

    // Load both entities at version 1
    let mut e1_a = repo.get_one("e1").unwrap().unwrap();
    let mut e2_a = repo.get_one("e2").unwrap().unwrap();

    // Concurrently modify e2 from another "session"
    let mut e2_b = repo.get_one("e2").unwrap().unwrap();
    e2_b.digest("Conflict", &"b");
    repo.commit(&mut e2_b).unwrap();

    // Try to commit both e1_a and e2_a together
    // e1 would be fine, but e2 has a version conflict
    e1_a.digest("Update", &"a");
    e2_a.digest("Update", &"a");
    let err = repo.commit(&mut [&mut e1_a, &mut e2_a]).unwrap_err();
    match err {
        sourced_rust::RepositoryError::ConcurrentWrite { id, .. } => {
            assert_eq!(id, "e2");
        }
        other => panic!("expected ConcurrentWrite, got: {:?}", other),
    }

    // e1 should NOT have been modified (atomic rollback - phase 1 validates all before writing)
    let e1_loaded = repo.get_one("e1").unwrap().unwrap();
    assert_eq!(e1_loaded.events().len(), 1);
    let e2_loaded = repo.get_one("e2").unwrap().unwrap();
    assert_eq!(e2_loaded.events().len(), 2);
}

// --- Version Tracking ---

#[test]
fn new_entity_has_zero_versions() {
    let entity = Entity::with_id("e1");
    assert_eq!(entity.committed_version(), 0);
    assert_eq!(entity.snapshot_version(), 0);
    assert!(entity.new_events().is_empty());
}

#[test]
fn load_from_history_sets_committed_version() {
    let repo = HashMapRepository::new();

    let mut entity = Entity::with_id("e1");
    entity.digest("Created", &"v1");
    entity.digest("Updated", &"v2");
    repo.commit(&mut entity).unwrap();

    let loaded = repo.get_one("e1").unwrap().unwrap();
    assert_eq!(loaded.committed_version(), 2);
    assert_eq!(loaded.snapshot_version(), 0);
    assert_eq!(loaded.version(), 2);
    assert!(loaded.new_events().is_empty());
}

#[test]
fn commit_updates_committed_version() {
    let repo = HashMapRepository::new();

    let mut entity = Entity::with_id("e1");
    entity.digest("Created", &"v1");
    assert_eq!(entity.committed_version(), 0);

    repo.commit(&mut entity).unwrap();
    assert_eq!(entity.committed_version(), 1);
    assert_eq!(entity.snapshot_version(), 0);
    assert!(entity.new_events().is_empty());

    // Add more events and commit again
    entity.digest("Updated", &"v2");
    assert_eq!(entity.new_events().len(), 1);

    repo.commit(&mut entity).unwrap();
    assert_eq!(entity.committed_version(), 2);
    assert_eq!(entity.snapshot_version(), 0);
    assert!(entity.new_events().is_empty());
}

#[test]
fn new_events_returns_only_uncommitted() {
    let repo = HashMapRepository::new();

    let mut entity = Entity::with_id("e1");
    entity.digest("e1", &"a");
    entity.digest("e2", &"b");
    assert_eq!(entity.new_events().len(), 2);

    repo.commit(&mut entity).unwrap();
    assert!(entity.new_events().is_empty());

    entity.digest("e3", &"c");
    assert_eq!(entity.new_events().len(), 1);
    assert_eq!(entity.new_events()[0].event_name, "e3");
}
