//! Integration tests for read models (ReadModel + ReadModelStore).

mod aggregate;
mod views;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use aggregate::Counter;
use sourced_rust::{
    AggregateBuilder, CommitBuilderExt, HashMapRepository, OutboxMessage, QueuedReadModelStore,
    ReadModelsExt, ReadOpts,
};
use views::{CounterView, UserCountersIndexView};

#[test]
fn readmodel_commits_with_aggregate() {
    let repo = HashMapRepository::new();

    // Create and modify aggregate
    let mut counter = Counter::new();
    counter.create("counter-1".into(), "Page Views".into(), "user-1".into());
    counter.increment(5);

    // Create read model from aggregate state
    let mut view = CounterView::new("counter-1", "Page Views", "user-1");
    view.set_value(counter.value());

    // Create outbox message to broadcast the change
    let outbox = OutboxMessage::encode("counter-1:created", "CounterCreated", &view).unwrap();

    // Commit read model, outbox, and aggregate together
    repo.readmodel(&view)
        .outbox(outbox)
        .commit(&mut counter)
        .unwrap();

    // Verify aggregate was stored
    let agg_repo = repo.clone().aggregate::<Counter>();
    let stored = agg_repo.get("counter-1").unwrap();
    assert!(stored.is_some());
    assert_eq!(stored.unwrap().value(), 5);

    // Verify read model was stored
    let stored_view = repo
        .read_models::<CounterView>()
        .get("counter-1")
        .unwrap()
        .unwrap();
    assert_eq!(stored_view.data.value, 5);
    assert_eq!(stored_view.data.name, "Page Views");
}

#[test]
fn multiple_readmodels_commit_together() {
    let repo = HashMapRepository::new();

    // Create aggregate
    let mut counter = Counter::new();
    counter.create("counter-2".into(), "Clicks".into(), "user-abc".into());
    counter.increment(10);

    // Create counter view read model
    let mut counter_view = CounterView::new("counter-2", "Clicks", "user-abc");
    counter_view.set_value(counter.value());

    // Create user index read model
    let mut user_index = UserCountersIndexView::new("user-abc");
    user_index.add_counter("counter-2", counter.value());

    // Create outbox message
    let outbox =
        OutboxMessage::encode("counter-2:created", "CounterCreated", &counter_view).unwrap();

    // Commit all together
    repo.readmodel(&counter_view)
        .readmodel(&user_index)
        .outbox(outbox)
        .commit(&mut counter)
        .unwrap();

    // Verify read models stored
    let view = repo
        .read_models::<CounterView>()
        .get("counter-2")
        .unwrap()
        .unwrap();
    assert_eq!(view.data.value, 10);

    let index = repo
        .read_models::<UserCountersIndexView>()
        .get("user-abc")
        .unwrap()
        .unwrap();
    assert!(index.data.counter_ids.contains(&"counter-2".to_string()));
    assert_eq!(index.data.total_value, 10);
}

#[test]
fn readmodel_update_with_outbox() {
    let repo = HashMapRepository::new();

    // Initial creation
    let mut counter = Counter::new();
    counter.create("counter-3".into(), "Downloads".into(), "user-xyz".into());

    let view = CounterView::new("counter-3", "Downloads", "user-xyz");

    let create_outbox = OutboxMessage::encode("counter-3:v1", "CounterCreated", &view).unwrap();

    repo.readmodel(&view)
        .outbox(create_outbox)
        .commit(&mut counter)
        .unwrap();

    // Now increment and update
    counter.increment(3);

    let mut loaded_view = repo
        .read_models::<CounterView>()
        .get("counter-3")
        .unwrap()
        .unwrap()
        .data;
    loaded_view.set_value(counter.value());

    let update_outbox =
        OutboxMessage::encode("counter-3:v2", "CounterUpdated", &loaded_view).unwrap();

    repo.readmodel(&loaded_view)
        .outbox(update_outbox)
        .commit(&mut counter)
        .unwrap();

    // Verify final state
    let final_view = repo
        .read_models::<CounterView>()
        .get("counter-3")
        .unwrap()
        .unwrap();
    assert_eq!(final_view.data.value, 3);
}

#[test]
fn readmodel_load_and_update() {
    let repo = HashMapRepository::new();

    // Initial commit
    let mut counter = Counter::new();
    counter.create("counter-4".into(), "Likes".into(), "user-456".into());

    let view = CounterView::new("counter-4", "Likes", "user-456");

    let outbox = OutboxMessage::encode("counter-4:created", "CounterCreated", &view).unwrap();

    repo.readmodel(&view)
        .outbox(outbox)
        .commit(&mut counter)
        .unwrap();

    // Load read model and verify initial state
    let loaded = repo
        .read_models::<CounterView>()
        .get("counter-4")
        .unwrap()
        .unwrap();
    assert_eq!(loaded.data.value, 0);
    assert_eq!(loaded.version, 1);

    // Modify aggregate and update read model
    counter.increment(7);
    let mut updated_view = loaded.data;
    updated_view.set_value(counter.value());

    let update_outbox =
        OutboxMessage::encode("counter-4:updated", "CounterUpdated", &updated_view).unwrap();

    // Commit updated read model
    repo.readmodel(&updated_view)
        .outbox(update_outbox)
        .commit(&mut counter)
        .unwrap();

    // Load again and verify updated state
    let final_view = repo
        .read_models::<CounterView>()
        .get("counter-4")
        .unwrap()
        .unwrap();
    assert_eq!(final_view.data.value, 7);
    assert_eq!(final_view.version, 2);
}

#[test]
fn get_readmodel_returns_none_for_missing() {
    let repo = HashMapRepository::new();

    let result = repo
        .read_models::<CounterView>()
        .get("nonexistent")
        .unwrap();

    assert!(result.is_none());
}

#[test]
fn commit_all_without_aggregate() {
    let repo = HashMapRepository::new();

    let view1 = CounterView::new("standalone-1", "View 1", "user-1");
    let view2 = CounterView::new("standalone-2", "View 2", "user-2");

    repo.readmodel(&view1)
        .readmodel(&view2)
        .commit_all()
        .unwrap();

    let loaded1 = repo
        .read_models::<CounterView>()
        .get("standalone-1")
        .unwrap()
        .unwrap();
    let loaded2 = repo
        .read_models::<CounterView>()
        .get("standalone-2")
        .unwrap()
        .unwrap();
    assert_eq!(loaded1.data.id, "standalone-1");
    assert_eq!(loaded2.data.id, "standalone-2");
}

#[test]
fn outbox_then_readmodel_order() {
    let repo = HashMapRepository::new();

    let mut counter = Counter::new();
    counter.create("counter-5".into(), "Shares".into(), "user-999".into());
    counter.increment(42);

    let mut view = CounterView::new("counter-5", "Shares", "user-999");
    view.set_value(counter.value());

    let outbox = OutboxMessage::encode("counter-5:created", "CounterCreated", &view).unwrap();

    // Outbox THEN read model — order shouldn't matter
    repo.outbox(outbox)
        .readmodel(&view)
        .commit(&mut counter)
        .unwrap();

    let stored_view = repo
        .read_models::<CounterView>()
        .get("counter-5")
        .unwrap()
        .unwrap();
    assert_eq!(stored_view.data.value, 42);
}

#[test]
fn standalone_readmodel_crud() {
    let repo = HashMapRepository::new();

    // Upsert directly via read_models()
    let view = CounterView::new("direct-1", "Direct", "user-direct");
    repo.read_models::<CounterView>().upsert(&view).unwrap();

    // Load back
    let loaded = repo
        .read_models::<CounterView>()
        .get("direct-1")
        .unwrap()
        .unwrap();
    assert_eq!(loaded.data.name, "Direct");
    assert_eq!(loaded.version, 1);

    // Update
    let mut updated = loaded.data;
    updated.set_value(100);
    repo.read_models::<CounterView>().upsert(&updated).unwrap();

    let reloaded = repo
        .read_models::<CounterView>()
        .get("direct-1")
        .unwrap()
        .unwrap();
    assert_eq!(reloaded.data.value, 100);
    assert_eq!(reloaded.version, 2);

    // Delete
    assert!(repo
        .read_models::<CounterView>()
        .delete("direct-1")
        .unwrap());
    assert!(repo
        .read_models::<CounterView>()
        .get("direct-1")
        .unwrap()
        .is_none());
}

// ============================================================================
// QueuedReadModelStore tests
// ============================================================================

#[test]
fn queued_readmodel_get_locks_commit_unlocks() {
    let store = QueuedReadModelStore::new(HashMapRepository::new());

    // Create aggregate and seed read model
    let mut counter = Counter::new();
    counter.create("q-1".into(), "Queued".into(), "user-q".into());
    counter.increment(5);

    let mut view = CounterView::new("q-1", "Queued", "user-q");
    view.set_value(counter.value());

    store.readmodel(&view).commit(&mut counter).unwrap();

    // get locks the read model
    let loaded = store
        .read_models::<CounterView>()
        .get("q-1")
        .unwrap()
        .unwrap();
    assert_eq!(loaded.data.value, 5);

    // Modify and commit — releases the lock
    counter.increment(3);
    let mut updated = loaded.data;
    updated.set_value(counter.value());

    store.readmodel(&updated).commit(&mut counter).unwrap();

    // Can get again (lock was released by commit)
    let final_view = store
        .read_models::<CounterView>()
        .get("q-1")
        .unwrap()
        .unwrap();
    assert_eq!(final_view.data.value, 8);

    // Cleanup
    store.unlock::<CounterView>("q-1").unwrap();
}

#[test]
fn queued_readmodel_upsert_releases_lock() {
    let store = QueuedReadModelStore::new(HashMapRepository::new());

    let view = CounterView::new("q-2", "Direct", "user-q");
    store.read_models::<CounterView>().upsert(&view).unwrap();

    // get locks
    let loaded = store
        .read_models::<CounterView>()
        .get("q-2")
        .unwrap()
        .unwrap();
    assert_eq!(loaded.data.value, 0);

    // upsert releases
    let mut updated = loaded.data;
    updated.set_value(42);
    store.read_models::<CounterView>().upsert(&updated).unwrap();

    // Can get again
    let reloaded = store
        .read_models::<CounterView>()
        .get("q-2")
        .unwrap()
        .unwrap();
    assert_eq!(reloaded.data.value, 42);
    store.unlock::<CounterView>("q-2").unwrap();
}

#[test]
fn queued_readmodel_abort_releases_lock() {
    let store = QueuedReadModelStore::new(HashMapRepository::new());

    let view = CounterView::new("q-3", "Abort", "user-q");
    store.read_models::<CounterView>().upsert(&view).unwrap();

    // get locks
    let _loaded = store.read_models::<CounterView>().get("q-3").unwrap();

    // Decide not to update — abort releases
    store.abort::<CounterView>("q-3").unwrap();

    // Can get again
    let reloaded = store
        .read_models::<CounterView>()
        .get("q-3")
        .unwrap()
        .unwrap();
    assert_eq!(reloaded.data.name, "Abort");
    store.unlock::<CounterView>("q-3").unwrap();
}

#[test]
fn queued_readmodel_no_lock_peek() {
    let store = QueuedReadModelStore::new(HashMapRepository::new());

    let view = CounterView::new("q-4", "Peek", "user-q");
    store.read_models::<CounterView>().upsert(&view).unwrap();

    // get with lock
    let _loaded = store.read_models::<CounterView>().get("q-4").unwrap();

    // Peek without lock — doesn't block
    let peeked = store
        .get_model_with::<CounterView>("q-4", ReadOpts::no_lock())
        .unwrap()
        .unwrap();
    assert_eq!(peeked.data.name, "Peek");

    // cleanup
    store.unlock::<CounterView>("q-4").unwrap();
}

#[test]
fn queued_readmodel_concurrent_increments() {
    let store = Arc::new(QueuedReadModelStore::new(HashMapRepository::new()));

    // Seed
    let view = CounterView::new("q-5", "Concurrent", "user-q");
    store.read_models::<CounterView>().upsert(&view).unwrap();

    let store2 = store.clone();

    // Thread 1: get (lock), sleep, increment, upsert (unlock)
    let t1 = thread::spawn(move || {
        let loaded = store2
            .read_models::<CounterView>()
            .get("q-5")
            .unwrap()
            .unwrap();
        thread::sleep(Duration::from_millis(50));
        let mut updated = loaded.data;
        updated.set_value(updated.value + 1);
        store2
            .read_models::<CounterView>()
            .upsert(&updated)
            .unwrap();
    });

    // Small delay so t1 acquires lock first
    thread::sleep(Duration::from_millis(10));

    // Main thread: get (waits for t1), increment, upsert
    let loaded = store
        .read_models::<CounterView>()
        .get("q-5")
        .unwrap()
        .unwrap();
    let mut updated = loaded.data;
    updated.set_value(updated.value + 1);
    store.read_models::<CounterView>().upsert(&updated).unwrap();

    t1.join().unwrap();

    // Both increments applied — no lost update
    let final_view = store
        .get_model_with::<CounterView>("q-5", ReadOpts::no_lock())
        .unwrap()
        .unwrap();
    assert_eq!(final_view.data.value, 2);
}

#[test]
fn queued_readmodel_different_types_no_contention() {
    let store = QueuedReadModelStore::new(HashMapRepository::new());

    // Seed both types with same logical ID
    let view = CounterView::new("shared-id", "Counter", "user-1");
    let index = UserCountersIndexView::new("shared-id");

    store.read_models::<CounterView>().upsert(&view).unwrap();
    store
        .read_models::<UserCountersIndexView>()
        .upsert(&index)
        .unwrap();

    // Lock CounterView "shared-id"
    let _cv = store.read_models::<CounterView>().get("shared-id").unwrap();

    // UserCountersIndexView "shared-id" is NOT blocked (different collection)
    let idx = store
        .read_models::<UserCountersIndexView>()
        .get("shared-id")
        .unwrap()
        .unwrap();
    assert_eq!(idx.data.user_id, "shared-id");

    // cleanup
    store.unlock::<CounterView>("shared-id").unwrap();
    store.unlock::<UserCountersIndexView>("shared-id").unwrap();
}

#[test]
fn queued_readmodel_full_lifecycle_with_aggregate_and_outbox() {
    let store = Arc::new(QueuedReadModelStore::new(HashMapRepository::new()));

    // ── Step 1: Create new aggregate, read model, and outbox ──
    let mut counter = Counter::new();
    counter.create("life-1".into(), "Lifecycle".into(), "user-life".into());
    counter.increment(10);

    let mut view = CounterView::new("life-1", "Lifecycle", "user-life");
    view.set_value(counter.value());

    let mut index = UserCountersIndexView::new("user-life");
    index.add_counter("life-1", counter.value());

    let outbox = OutboxMessage::encode("life-1:created", "CounterCreated", &view).unwrap();

    // ── Step 2: Commit aggregate + both read models + outbox atomically ──
    store
        .readmodel(&view)
        .readmodel(&index)
        .outbox(outbox)
        .commit(&mut counter)
        .unwrap();

    // ── Step 3: Get the committed read models back (locks them) ──
    let loaded_view = store
        .read_models::<CounterView>()
        .get("life-1")
        .unwrap()
        .unwrap();
    assert_eq!(loaded_view.data.value, 10);

    let loaded_index = store
        .read_models::<UserCountersIndexView>()
        .get("user-life")
        .unwrap()
        .unwrap();
    assert_eq!(loaded_index.data.total_value, 10);

    // ── Step 4: Assert they are locked ──
    // Spawn threads that try to get the same read models — they should block
    let view_locked = Arc::new(AtomicBool::new(true));
    let index_locked = Arc::new(AtomicBool::new(true));

    let store_t1 = store.clone();
    let flag_t1 = view_locked.clone();
    let t1 = thread::spawn(move || {
        // This will block until the lock is released
        let _v = store_t1.read_models::<CounterView>().get("life-1").unwrap();
        flag_t1.store(false, Ordering::SeqCst);
        // Release immediately so test doesn't deadlock
        store_t1.unlock::<CounterView>("life-1").unwrap();
    });

    let store_t2 = store.clone();
    let flag_t2 = index_locked.clone();
    let t2 = thread::spawn(move || {
        let _i = store_t2
            .read_models::<UserCountersIndexView>()
            .get("user-life")
            .unwrap();
        flag_t2.store(false, Ordering::SeqCst);
        store_t2
            .unlock::<UserCountersIndexView>("user-life")
            .unwrap();
    });

    // Give threads time to attempt acquisition — they should be blocked
    thread::sleep(Duration::from_millis(50));
    assert!(
        view_locked.load(Ordering::SeqCst),
        "CounterView should still be locked"
    );
    assert!(
        index_locked.load(Ordering::SeqCst),
        "UserCountersIndexView should still be locked"
    );

    // ── Step 5: Update the aggregate and read models ──
    counter.increment(5);

    let mut updated_view = loaded_view.data;
    updated_view.set_value(counter.value());

    let mut updated_index = loaded_index.data;
    updated_index.add_counter("life-1", 5); // adding the delta

    let update_outbox =
        OutboxMessage::encode("life-1:updated", "CounterUpdated", &updated_view).unwrap();

    // ── Step 6: Commit updates (releases the locks) ──
    store
        .readmodel(&updated_view)
        .readmodel(&updated_index)
        .outbox(update_outbox)
        .commit(&mut counter)
        .unwrap();

    // ── Step 7: Assert they are unlocked ──
    // The spawned threads should now complete
    t1.join().unwrap();
    t2.join().unwrap();

    assert!(
        !view_locked.load(Ordering::SeqCst),
        "CounterView should be unlocked"
    );
    assert!(
        !index_locked.load(Ordering::SeqCst),
        "UserCountersIndexView should be unlocked"
    );

    // Verify final state via no-lock peek
    let final_view = store
        .get_model_with::<CounterView>("life-1", ReadOpts::no_lock())
        .unwrap()
        .unwrap();
    assert_eq!(final_view.data.value, 15);
    assert_eq!(final_view.data.name, "Lifecycle");

    let final_index = store
        .get_model_with::<UserCountersIndexView>("user-life", ReadOpts::no_lock())
        .unwrap()
        .unwrap();
    assert_eq!(final_index.data.total_value, 15);
    assert!(final_index.data.counter_ids.contains(&"life-1".to_string()));

    // Also verify the aggregate persisted correctly
    let agg_repo = store.inner().clone().aggregate::<Counter>();
    let stored_counter = agg_repo.get("life-1").unwrap().unwrap();
    assert_eq!(stored_counter.value(), 15);
}
