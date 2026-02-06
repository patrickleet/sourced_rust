//! Integration tests for read models (ReadModel + ReadModelStore).

mod aggregate;
mod views;

use sourced_rust::{
    AggregateBuilder, CommitBuilderExt, HashMapRepository, ReadModelsExt, OutboxMessage,
};
use aggregate::Counter;
use views::{CounterView, UserCountersIndex};

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
    let outbox =
        OutboxMessage::encode("counter-1:created", "CounterCreated", &view).unwrap();

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
    let mut user_index = UserCountersIndex::new("user-abc");
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
        .read_models::<UserCountersIndex>()
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

    let create_outbox =
        OutboxMessage::encode("counter-3:v1", "CounterCreated", &view).unwrap();

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

    let outbox =
        OutboxMessage::encode("counter-4:created", "CounterCreated", &view).unwrap();

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

    let outbox =
        OutboxMessage::encode("counter-5:created", "CounterCreated", &view).unwrap();

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
    assert!(repo.read_models::<CounterView>().delete("direct-1").unwrap());
    assert!(repo.read_models::<CounterView>().get("direct-1").unwrap().is_none());
}
