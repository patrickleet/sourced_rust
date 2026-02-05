//! Integration tests for projections.

mod aggregate;
mod repository;
mod views;

use sourced_rust::{CommitBuilderExt, OutboxMessage, Projection, ProjectionsExt};
use aggregate::Counter;
use repository::CounterRepository;
use views::{CounterView, UserCountersIndex};

#[test]
fn projection_commits_with_aggregate() {
    let repo = CounterRepository::new();

    // Create and modify aggregate
    let mut counter = Counter::new();
    counter.create("counter-1".into(), "Page Views".into(), "user-1".into());
    counter.increment(5);

    // Create projection from aggregate state
    let mut view = repo
        .repo()
        .projections()
        .upsert(CounterView::new("counter-1", "Page Views", "user-1"))
        .unwrap();
    view.data_mut().set_value(counter.value());

    // Create outbox message to broadcast the change
    let outbox =
        OutboxMessage::encode("counter-1:created", "CounterCreated", view.data()).unwrap();

    // Commit projection, outbox, and aggregate together
    repo.repo()
        .projection(view)
        .outbox(outbox)
        .commit(&mut counter)
        .unwrap();

    // Verify aggregate was stored
    let stored = repo.get("counter-1").unwrap();
    assert!(stored.is_some());
    assert_eq!(stored.unwrap().value(), 5);

    // Verify projection was stored
    let stored_view: Projection<CounterView> = repo
        .repo()
        .projections()
        .get("counter-1")
        .unwrap()
        .unwrap();
    assert_eq!(stored_view.data().value, 5);
    assert_eq!(stored_view.data().name, "Page Views");
}

#[test]
fn multiple_projections_commit_together() {
    let repo = CounterRepository::new();

    // Create aggregate
    let mut counter = Counter::new();
    counter.create("counter-2".into(), "Clicks".into(), "user-abc".into());
    counter.increment(10);

    // Create counter view projection
    let mut counter_view = repo
        .repo()
        .projections()
        .upsert(CounterView::new("counter-2", "Clicks", "user-abc"))
        .unwrap();
    counter_view.data_mut().set_value(counter.value());

    // Create user index projection
    let mut user_index = repo
        .repo()
        .projections()
        .upsert(UserCountersIndex::new("user-abc"))
        .unwrap();
    user_index.data_mut().add_counter("counter-2", counter.value());

    // Create outbox message
    let outbox =
        OutboxMessage::encode("counter-2:created", "CounterCreated", counter_view.data()).unwrap();

    // Commit all together
    repo.repo()
        .projection(counter_view)
        .projection(user_index)
        .outbox(outbox)
        .commit(&mut counter)
        .unwrap();

    // Verify projections stored
    let view: Projection<CounterView> = repo
        .repo()
        .projections()
        .get("counter-2")
        .unwrap()
        .unwrap();
    assert_eq!(view.data().value, 10);

    let index: Projection<UserCountersIndex> = repo
        .repo()
        .projections()
        .get("user-abc")
        .unwrap()
        .unwrap();
    assert!(index.data().counter_ids.contains(&"counter-2".to_string()));
    assert_eq!(index.data().total_value, 10);
}

#[test]
fn projection_update_with_outbox() {
    let repo = CounterRepository::new();

    // Initial creation
    let mut counter = Counter::new();
    counter.create("counter-3".into(), "Downloads".into(), "user-xyz".into());

    let view = repo
        .repo()
        .projections()
        .upsert(CounterView::new("counter-3", "Downloads", "user-xyz"))
        .unwrap();

    let create_outbox =
        OutboxMessage::encode("counter-3:v1", "CounterCreated", view.data()).unwrap();

    repo.repo()
        .projection(view)
        .outbox(create_outbox)
        .commit(&mut counter)
        .unwrap();

    // Now increment and update
    counter.increment(3);

    let mut loaded_view: Projection<CounterView> = repo
        .repo()
        .projections()
        .get("counter-3")
        .unwrap()
        .unwrap();
    loaded_view.data_mut().set_value(counter.value());

    let update_outbox =
        OutboxMessage::encode("counter-3:v2", "CounterUpdated", loaded_view.data()).unwrap();

    repo.repo()
        .projection(loaded_view)
        .outbox(update_outbox)
        .commit(&mut counter)
        .unwrap();

    // Verify final state
    let final_view: Projection<CounterView> = repo
        .repo()
        .projections()
        .get("counter-3")
        .unwrap()
        .unwrap();
    assert_eq!(final_view.data().value, 3);
}

#[test]
fn projection_load_and_update() {
    let repo = CounterRepository::new();

    // Initial commit
    let mut counter = Counter::new();
    counter.create("counter-4".into(), "Likes".into(), "user-456".into());

    let view = repo
        .repo()
        .projections()
        .upsert(CounterView::new("counter-4", "Likes", "user-456"))
        .unwrap();

    let outbox =
        OutboxMessage::encode("counter-4:created", "CounterCreated", view.data()).unwrap();

    repo.repo()
        .projection(view)
        .outbox(outbox)
        .commit(&mut counter)
        .unwrap();

    // Load projection and verify initial state
    let mut loaded_view: Projection<CounterView> = repo
        .repo()
        .projections()
        .get("counter-4")
        .unwrap()
        .unwrap();
    assert_eq!(loaded_view.data().value, 0);
    assert!(!loaded_view.is_dirty());

    // Modify aggregate and update projection
    counter.increment(7);
    loaded_view.data_mut().set_value(counter.value());
    assert!(loaded_view.is_dirty());

    let update_outbox =
        OutboxMessage::encode("counter-4:updated", "CounterUpdated", loaded_view.data()).unwrap();

    // Commit updated projection
    repo.repo()
        .projection(loaded_view)
        .outbox(update_outbox)
        .commit(&mut counter)
        .unwrap();

    // Load again and verify updated state
    let final_view: Projection<CounterView> = repo
        .repo()
        .projections()
        .get("counter-4")
        .unwrap()
        .unwrap();
    assert_eq!(final_view.data().value, 7);
}

#[test]
fn get_projection_returns_none_for_missing() {
    let repo = CounterRepository::new();

    let result: Option<Projection<CounterView>> =
        repo.repo().projections().get("nonexistent").unwrap();

    assert!(result.is_none());
}

#[test]
fn commit_all_without_aggregate() {
    let repo = CounterRepository::new();

    let view1 = Projection::new(CounterView::new("standalone-1", "View 1", "user-1"));
    let view2 = Projection::new(CounterView::new("standalone-2", "View 2", "user-2"));

    repo.repo()
        .projection(view1)
        .projection(view2)
        .commit_all()
        .unwrap();

    let loaded1: Projection<CounterView> = repo
        .repo()
        .projections()
        .get("standalone-1")
        .unwrap()
        .unwrap();
    let loaded2: Projection<CounterView> = repo
        .repo()
        .projections()
        .get("standalone-2")
        .unwrap()
        .unwrap();
    assert_eq!(loaded1.data().id, "standalone-1");
    assert_eq!(loaded2.data().id, "standalone-2");
}

#[test]
fn projection_into_data() {
    let view = CounterView::new("counter-5", "Test", "user-789");
    let proj = Projection::new(view);

    // Consume projection to get the data
    let data = proj.into_data();

    assert_eq!(data.id, "counter-5");
    assert_eq!(data.name, "Test");
    assert_eq!(data.user_id, "user-789");
}
