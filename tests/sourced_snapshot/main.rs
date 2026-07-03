mod aggregates;

use aggregates::*;
use distributed::{
    Aggregate, AggregateBuilder, HashMapRepository, OutboxMessage, OutboxStore, SnapshotStore,
    Snapshottable, StreamIdentity,
};

// ============================================================================
// Default case: id + all fields
// ============================================================================

#[test]
fn default_snapshot_has_id_and_all_fields() {
    let mut todo = Todo::new();
    todo.initialize("t1".into(), "alice".into(), "Buy milk".into())
        .unwrap();
    todo.complete().unwrap();

    let snap = todo.snapshot();
    assert_eq!(snap.id, "t1");
    assert_eq!(snap.user_id, "alice");
    assert_eq!(snap.task, "Buy milk");
    assert!(snap.completed);
}

#[tokio::test]
async fn default_snapshot_roundtrip_via_snapshottable() {
    let repo = HashMapRepository::new()
        .aggregate::<Todo>()
        .with_snapshots(1);

    let mut todo = Todo::new();
    todo.initialize("t1".into(), "alice".into(), "Buy milk".into())
        .unwrap();
    repo.commit(&mut todo).await.unwrap();

    let loaded = repo.get("t1").await.unwrap().unwrap();
    let snap = loaded.snapshot();
    assert_eq!(snap.id, "t1");
    assert_eq!(snap.user_id, "alice");
    assert_eq!(snap.task, "Buy milk");
    assert!(!snap.completed);
}

#[test]
fn default_restore_from_snapshot() {
    let mut todo = Todo::new();
    let snap = TodoSnapshot {
        id: "restored".into(),
        user_id: "bob".into(),
        task: "Walk dog".into(),
        completed: true,
    };
    todo.restore_from_snapshot(snap);
    assert_eq!(todo.entity.id(), "restored");
    assert_eq!(todo.user_id, "bob");
    assert_eq!(todo.task, "Walk dog");
    assert!(todo.completed);
}

// ============================================================================
// Custom ID key: snapshot(id = "sku")
// ============================================================================

#[test]
fn custom_id_snapshot_uses_field_as_key() {
    let mut inv = Inventory::new();
    inv.create("inv-1".into(), "WIDGET-42".into(), 100).unwrap();

    let snap = inv.snapshot();
    // The snapshot should have `sku` as the id field, not a separate `id`
    assert_eq!(snap.sku, "WIDGET-42");
    assert_eq!(snap.available, 100);
}

#[test]
fn custom_id_restore_sets_entity_id_from_field() {
    let mut inv = Inventory::new();
    let snap = InventorySnapshot {
        sku: "GADGET-99".into(),
        available: 50,
    };
    inv.restore_from_snapshot(snap);
    assert_eq!(inv.entity.id(), "GADGET-99");
    assert_eq!(inv.sku, "GADGET-99");
    assert_eq!(inv.available, 50);
}

#[tokio::test]
async fn custom_id_roundtrip_via_repo() {
    let repo = HashMapRepository::new()
        .aggregate::<Inventory>()
        .with_snapshots(1);

    let mut inv = Inventory::new();
    inv.create("inv-1".into(), "SKU-A".into(), 10).unwrap();
    repo.commit(&mut inv).await.unwrap();

    let loaded = repo.get("inv-1").await.unwrap().unwrap();
    let snap = loaded.snapshot();
    assert_eq!(snap.sku, "SKU-A");
    assert_eq!(snap.available, 10);
}

// ============================================================================
// serde(skip) field exclusion
// ============================================================================

#[test]
fn serde_skip_fields_excluded_from_snapshot() {
    let mut order = Order::new();
    order.place("o1".into(), "alice".into(), 999).unwrap();

    let snap = order.snapshot();
    assert_eq!(snap.id, "o1");
    assert_eq!(snap.customer, "alice");
    assert_eq!(snap.total, 999);
    // cached_label is not in OrderSnapshot - verified by compilation
}

#[test]
fn serde_skip_default_excluded_from_snapshot() {
    let mut notifier = Notifier::new();
    notifier.send("n1".into(), "hello".into()).unwrap();

    let snap = notifier.snapshot();
    assert_eq!(snap.id, "n1");
    assert_eq!(snap.message, "hello");
    // emitter is not in NotifierSnapshot - verified by compilation
}

#[tokio::test]
async fn serde_skip_restore_roundtrip() {
    let repo = HashMapRepository::new()
        .aggregate::<Order>()
        .with_snapshots(1);

    let mut order = Order::new();
    order.place("o1".into(), "alice".into(), 500).unwrap();
    repo.commit(&mut order).await.unwrap();

    let loaded = repo.get("o1").await.unwrap().unwrap();
    assert_eq!(loaded.snapshot().customer, "alice");
    assert_eq!(loaded.snapshot().total, 500);
    // cached_label will be default (empty) after restore, which is correct
    assert_eq!(loaded.cached_label, "");
}

// ============================================================================
// Works alongside #[sourced(entity)]
// ============================================================================

#[test]
fn sourced_attr_with_snapshot_derive() {
    let mut counter = Counter::new();
    counter.initialize("c1".into()).unwrap();
    counter.increment(5).unwrap();
    counter.increment(3).unwrap();

    let snap = counter.snapshot();
    assert_eq!(snap.id, "c1");
    assert_eq!(snap.count, 8);
}

#[tokio::test]
async fn sourced_attr_snapshot_roundtrip_via_repo() {
    let repo = HashMapRepository::new()
        .aggregate::<Counter>()
        .with_snapshots(2);

    let mut counter = Counter::new();
    counter.initialize("c1".into()).unwrap();
    counter.increment(10).unwrap();
    repo.commit(&mut counter).await.unwrap();

    // At version 2, should have a snapshot
    let identity = StreamIdentity::new(Counter::aggregate_type(), "c1").unwrap();
    let snap_record = repo.repo().get_snapshot(&identity).await.unwrap();
    assert!(snap_record.is_some());

    let loaded = repo.get("c1").await.unwrap().unwrap();
    assert_eq!(loaded.snapshot().count, 10);
}

// ============================================================================
// Custom entity field name
// ============================================================================

#[test]
fn custom_entity_field_snapshot() {
    let mut widget = Widget::new();
    widget.create("w1".into(), "Sprocket".into(), 2.5).unwrap();

    let snap = widget.snapshot();
    assert_eq!(snap.id, "w1");
    assert_eq!(snap.name, "Sprocket");
    assert_eq!(snap.weight, 2.5);
}

#[test]
fn custom_entity_field_restore() {
    let mut widget = Widget::new();
    let snap = WidgetSnapshot {
        id: "w2".into(),
        name: "Gear".into(),
        weight: 1.0,
    };
    widget.restore_from_snapshot(snap);
    assert_eq!(widget.my_entity.id(), "w2");
    assert_eq!(widget.name, "Gear");
    assert_eq!(widget.weight, 1.0);
}

// ============================================================================
// OutboxMessage::domain_event
// ============================================================================

#[test]
fn domain_event_derives_id_and_payload() {
    let mut todo = Todo::new();
    todo.initialize("t1".into(), "alice".into(), "Buy milk".into())
        .unwrap();

    let outbox = OutboxMessage::domain_event("todo.initialized", &todo).unwrap();
    assert_eq!(outbox.id(), "t1:todo.initialized:1");
    assert_eq!(outbox.event_type, "todo.initialized");

    let decoded: TodoSnapshot = outbox.decode().unwrap();
    assert_eq!(decoded.id, "t1");
    assert_eq!(decoded.user_id, "alice");
    assert_eq!(decoded.task, "Buy milk");
}

#[test]
fn domain_event_propagates_metadata() {
    let mut todo = Todo::new();
    todo.entity.set_correlation_id("req-abc");
    todo.entity.set_causation_id("cmd-create");
    todo.entity.set_meta("user_id", "u-42");
    todo.initialize("t1".into(), "alice".into(), "Buy milk".into())
        .unwrap();

    let outbox = OutboxMessage::domain_event("todo.initialized", &todo).unwrap();
    assert_eq!(outbox.correlation_id(), Some("req-abc"));
    assert_eq!(outbox.causation_id(), Some("cmd-create"));
    assert_eq!(outbox.meta("user_id"), Some("u-42"));
}

#[tokio::test]
async fn domain_event_commits_with_outbox() {
    let repo = HashMapRepository::new().aggregate::<Todo>();

    let mut todo = Todo::new();
    todo.initialize("t1".into(), "alice".into(), "Ship it".into())
        .unwrap();

    let outbox = OutboxMessage::domain_event("todo.initialized", &todo).unwrap();
    repo.outbox(outbox).commit(&mut todo).await.unwrap();

    let loaded = repo.get("t1").await.unwrap().unwrap();
    assert_eq!(loaded.snapshot().task, "Ship it");
    let pending = repo
        .repo()
        .outbox_store()
        .pending(usize::MAX)
        .await
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert!(pending[0].is_pending());
}
