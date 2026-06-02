mod aggregate;

use aggregate::{Notifier, NotifierEvent, Order, OrderEvent};
use distributed::{Aggregate, AsyncAggregateBuilder, HashMapRepository, Queueable};
use std::sync::mpsc;
use std::time::Duration;

// =============================================================================
// #[sourced(entity, enqueue)] — both fire from single #[event]
// =============================================================================

#[test]
fn digest_and_enqueue_both_fire() {
    let mut order = Order::default();
    order.create("order-1".into(), "alice".into()).unwrap();

    assert_eq!(order.entity.version(), 1);
    assert_eq!(order.emitter.queued_len(), 1);
}

#[test]
fn full_lifecycle_digest_and_enqueue() {
    let mut order = Order::default();
    order.create("order-1".into(), "alice".into()).unwrap();
    order.confirm().unwrap();
    order.ship().unwrap();

    assert_eq!(order.entity.version(), 3);
    assert_eq!(order.emitter.queued_len(), 3);
    assert_eq!(order.status, "shipped");
}

// =============================================================================
// Replay does not re-enqueue
// =============================================================================

#[tokio::test]
async fn replay_does_not_re_enqueue() {
    let repo = HashMapRepository::new()
        .queued_async()
        .async_aggregate::<Order>();

    let mut order = Order::default();
    order.create("order-1".into(), "alice".into()).unwrap();
    order.confirm().unwrap();
    order.emitter.emit_queued();

    repo.commit(&mut order).await.unwrap();

    let loaded = repo.get("order-1").await.unwrap().unwrap();
    assert_eq!(loaded.emitter.queued_len(), 0);
    assert_eq!(loaded.status, "confirmed");
    assert_eq!(loaded.entity.version(), 2);
}

// =============================================================================
// Emit fires listeners
// =============================================================================

#[test]
fn emit_fires_listeners() {
    let mut order = Order::default();

    let (tx, rx) = mpsc::channel();
    order.emitter.on("initialized", move |_: String| {
        tx.send(()).unwrap();
    });

    order.create("order-1".into(), "alice".into()).unwrap();
    order.emitter.emit_queued();

    rx.recv_timeout(Duration::from_secs(1))
        .expect("initialized callback never fired");
}

// =============================================================================
// Guards stay in sync
// =============================================================================

#[test]
fn guards_stay_in_sync_between_digest_and_enqueue() {
    let mut order = Order::default();
    order.create("order-1".into(), "alice".into()).unwrap();
    order.confirm().unwrap();
    order.confirm().unwrap(); // second confirm blocked by guard

    assert_eq!(order.entity.version(), 2);
    assert_eq!(order.emitter.queued_len(), 2);
}

// =============================================================================
// Typed event enum exists
// =============================================================================

#[test]
fn typed_event_enum_exists() {
    let created = OrderEvent::Initialized {
        order_id: "o-1".into(),
        customer: "alice".into(),
    };
    assert_eq!(created.event_name(), "initialized");
    assert_eq!(OrderEvent::Confirmed.event_name(), "confirmed");
    assert_eq!(OrderEvent::Shipped.event_name(), "shipped");
}

// =============================================================================
// Custom emitter field: enqueue(my_emitter)
// =============================================================================

#[test]
fn custom_emitter_field_enqueues() {
    let mut notifier = Notifier::default();
    notifier.send("n-1".into(), "Hello world".into()).unwrap();

    assert_eq!(notifier.entity.version(), 1);
    assert_eq!(notifier.my_emitter.queued_len(), 1);
}

#[test]
fn custom_emitter_field_emits() {
    let mut notifier = Notifier::default();

    let (tx, rx) = mpsc::channel();
    notifier.my_emitter.on("sent", move |_: String| {
        tx.send(()).unwrap();
    });

    notifier.send("n-1".into(), "Hello".into()).unwrap();
    notifier.my_emitter.emit_queued();

    rx.recv_timeout(Duration::from_secs(1))
        .expect("NotificationSent callback never fired");
}

#[tokio::test]
async fn custom_emitter_replay_does_not_enqueue() {
    let repo = HashMapRepository::new()
        .queued_async()
        .async_aggregate::<Notifier>();

    let mut notifier = Notifier::default();
    notifier.send("n-1".into(), "Hello".into()).unwrap();
    notifier.my_emitter.emit_queued();
    repo.commit(&mut notifier).await.unwrap();

    let loaded = repo.get("n-1").await.unwrap().unwrap();
    assert_eq!(loaded.my_emitter.queued_len(), 0);
    assert_eq!(loaded.message, "Hello");
}

#[test]
fn custom_emitter_typed_enum() {
    let event = NotifierEvent::Sent {
        id: "n-1".into(),
        message: "Hello".into(),
    };
    assert_eq!(event.event_name(), "sent");
}

#[test]
fn notifier_has_no_upcasters() {
    assert!(Notifier::upcasters().is_empty());
}
