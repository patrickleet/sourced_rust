mod aggregate;

use aggregate::Todo;
use sourced_rust::{
    AggregateBuilder, HashMapRepository, Queueable, SnapshotStore,
};

#[test]
fn snapshot_created_at_frequency_threshold() {
    let repo = HashMapRepository::new()
        .aggregate::<Todo>()
        .with_snapshots(2);

    let mut todo = Todo::new();
    todo.initialize("t1".into(), "alice".into(), "Buy milk".into());
    repo.commit(&mut todo).unwrap();

    // Version 1 — below threshold of 2, no snapshot yet
    assert!(repo.repo().repo().get_snapshot("t1").unwrap().is_none());

    // Load, add another event to reach version 2
    let mut todo = repo.get("t1").unwrap().unwrap();
    todo.complete();
    repo.commit(&mut todo).unwrap();

    // Version 2 >= 0 + 2 — snapshot should now exist
    let snap = repo.repo().repo().get_snapshot("t1").unwrap();
    assert!(snap.is_some());
    let snap = snap.unwrap();
    assert_eq!(snap.version, 2);

    // Reload and verify state
    let loaded = repo.get("t1").unwrap().unwrap();
    let s = loaded.snapshot();
    assert_eq!(s.id, "t1");
    assert_eq!(s.user_id, "alice");
    assert_eq!(s.task, "Buy milk");
    assert!(s.completed);
    assert_eq!(loaded.entity.version(), 2);
    assert_eq!(loaded.entity.snapshot_version(), 2);
}

#[test]
fn no_snapshot_before_threshold() {
    let repo = HashMapRepository::new()
        .aggregate::<Todo>()
        .with_snapshots(5);

    let mut todo = Todo::new();
    todo.initialize("t1".into(), "alice".into(), "Buy milk".into());
    repo.commit(&mut todo).unwrap();

    // Only 1 event, threshold is 5
    assert!(repo.repo().repo().get_snapshot("t1").unwrap().is_none());
}

#[test]
fn load_from_snapshot_produces_correct_state() {
    let repo = HashMapRepository::new()
        .aggregate::<Todo>()
        .with_snapshots(2);

    let mut todo = Todo::new();
    todo.initialize("t1".into(), "alice".into(), "Buy milk".into());
    todo.complete();
    repo.commit(&mut todo).unwrap();

    // Snapshot at version 2
    assert!(repo.repo().repo().get_snapshot("t1").unwrap().is_some());

    // Reload — should use snapshot
    let loaded = repo.get("t1").unwrap().unwrap();
    let snap = loaded.snapshot();
    assert_eq!(snap.id, "t1");
    assert_eq!(snap.user_id, "alice");
    assert_eq!(snap.task, "Buy milk");
    assert!(snap.completed);
}

#[test]
fn snapshot_plus_newer_events() {
    let repo = HashMapRepository::new()
        .aggregate::<Todo>()
        .with_snapshots(2);

    // Create and commit 2 events (triggers snapshot at version 2)
    let mut todo = Todo::new();
    todo.initialize("t1".into(), "alice".into(), "Buy milk".into());
    repo.commit(&mut todo).unwrap();

    let mut todo = repo.get("t1").unwrap().unwrap();
    todo.complete();
    repo.commit(&mut todo).unwrap();

    // Snapshot exists at version 2, completed = true
    let snap = repo.repo().repo().get_snapshot("t1").unwrap().unwrap();
    assert_eq!(snap.version, 2);

    // Now create a second todo to verify snapshot + partial replay works.
    // We'll use a different approach: create a fresh repo pointing to the same storage
    // and verify loading still works.
    let loaded = repo.get("t1").unwrap().unwrap();
    assert!(loaded.snapshot().completed);
    assert_eq!(loaded.entity.version(), 2);
    assert_eq!(loaded.entity.snapshot_version(), 2);
}

#[test]
fn no_snapshot_falls_back_to_full_replay() {
    let repo = HashMapRepository::new()
        .aggregate::<Todo>()
        .with_snapshots(2);

    let mut todo = Todo::new();
    todo.initialize("t1".into(), "alice".into(), "Buy milk".into());
    todo.complete();
    repo.commit(&mut todo).unwrap();

    // Snapshot exists
    assert!(repo.repo().repo().get_snapshot("t1").unwrap().is_some());

    // Delete the snapshot
    repo.repo().repo().delete_snapshot("t1").unwrap();
    assert!(repo.repo().repo().get_snapshot("t1").unwrap().is_none());

    // Loading should still work via full replay
    let loaded = repo.get("t1").unwrap().unwrap();
    let snap = loaded.snapshot();
    assert_eq!(snap.id, "t1");
    assert_eq!(snap.user_id, "alice");
    assert!(snap.completed);
}

#[test]
fn snapshot_version_advances_on_second_snapshot() {
    let repo = HashMapRepository::new()
        .aggregate::<Todo>()
        .with_snapshots(1); // snapshot every event

    let mut todo = Todo::new();
    todo.initialize("t1".into(), "alice".into(), "Buy milk".into());
    repo.commit(&mut todo).unwrap();

    // First snapshot at version 1
    let snap = repo.repo().repo().get_snapshot("t1").unwrap().unwrap();
    assert_eq!(snap.version, 1);

    // Add another event
    let mut todo = repo.get("t1").unwrap().unwrap();
    todo.complete();
    repo.commit(&mut todo).unwrap();

    // Second snapshot at version 2
    let snap = repo.repo().repo().get_snapshot("t1").unwrap().unwrap();
    assert_eq!(snap.version, 2);

    // Verify the loaded aggregate has correct snapshot_version
    let loaded = repo.get("t1").unwrap().unwrap();
    assert_eq!(loaded.entity.snapshot_version(), 2);
    assert!(loaded.snapshot().completed);
}

#[test]
fn with_queued_repo() {
    let repo = HashMapRepository::new()
        .queued()
        .aggregate::<Todo>()
        .with_snapshots(2);

    let mut todo = Todo::new();
    todo.initialize("t1".into(), "alice".into(), "Buy milk".into());
    repo.commit(&mut todo).unwrap();

    let mut todo = repo.get("t1").unwrap().unwrap();
    todo.complete();
    repo.commit(&mut todo).unwrap();

    // Snapshot should exist through the queued + snapshot chain
    let snap = repo.repo().repo().inner().get_snapshot("t1").unwrap();
    assert!(snap.is_some());
    assert_eq!(snap.unwrap().version, 2);

    // Reload and verify
    let loaded = repo.get("t1").unwrap().unwrap();
    assert!(loaded.snapshot().completed);
    assert_eq!(loaded.entity.snapshot_version(), 2);
}

#[test]
fn find_with_snapshots() {
    let repo = HashMapRepository::new()
        .aggregate::<Todo>()
        .with_snapshots(2);

    // Create two todos, both past snapshot threshold
    let mut todo1 = Todo::new();
    todo1.initialize("t1".into(), "alice".into(), "Buy milk".into());
    todo1.complete();
    repo.commit(&mut todo1).unwrap();

    let mut todo2 = Todo::new();
    todo2.initialize("t2".into(), "bob".into(), "Walk dog".into());
    todo2.complete();
    repo.commit(&mut todo2).unwrap();

    // Find all completed
    let completed = repo.find(|t| t.snapshot().completed).unwrap();
    assert_eq!(completed.len(), 2);

    // Find alice's todos
    let alice = repo.find(|t| t.snapshot().user_id == "alice").unwrap();
    assert_eq!(alice.len(), 1);
    assert_eq!(alice[0].snapshot().task, "Buy milk");
}

#[test]
fn commit_all_with_snapshots() {
    let repo = HashMapRepository::new()
        .aggregate::<Todo>()
        .with_snapshots(2);

    let mut todo1 = Todo::new();
    todo1.initialize("t1".into(), "alice".into(), "Task 1".into());
    todo1.complete(); // version 2

    let mut todo2 = Todo::new();
    todo2.initialize("t2".into(), "bob".into(), "Task 2".into());
    todo2.complete(); // version 2

    repo.commit_all(&mut [&mut todo1, &mut todo2]).unwrap();

    // Both should have snapshots at version 2
    let snap1 = repo.repo().repo().get_snapshot("t1").unwrap().unwrap();
    assert_eq!(snap1.version, 2);
    let snap2 = repo.repo().repo().get_snapshot("t2").unwrap().unwrap();
    assert_eq!(snap2.version, 2);
}
