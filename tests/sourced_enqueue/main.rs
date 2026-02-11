mod aggregate;

use aggregate::{Notifier, NotifierEvent, Order, OrderEvent};
use sourced_rust::{Aggregate, AggregateBuilder, HashMapRepository, Queueable};
use std::sync::mpsc;
use std::time::Duration;

// =============================================================================
// #[sourced(entity, enqueue)] — both fire from single #[event]
// =============================================================================

#[test]
fn digest_and_enqueue_both_fire() {
    let mut order = Order::default();
    order.create("order-1".into(), "alice".into());

    assert_eq!(order.entity.version(), 1);
    assert_eq!(order.emitter.queued_len(), 1);
}

#[test]
fn full_lifecycle_digest_and_enqueue() {
    let mut order = Order::default();
    order.create("order-1".into(), "alice".into());
    order.confirm();
    order.ship();

    assert_eq!(order.entity.version(), 3);
    assert_eq!(order.emitter.queued_len(), 3);
    assert_eq!(order.status, "shipped");
}

// =============================================================================
// Replay does not re-enqueue
// =============================================================================

#[test]
fn replay_does_not_re_enqueue() {
    let repo = HashMapRepository::new().queued().aggregate::<Order>();

    let mut order = Order::default();
    order.create("order-1".into(), "alice".into());
    order.confirm();
    order.emitter.emit_queued();

    repo.commit(&mut order).unwrap();

    let loaded = repo.get("order-1").unwrap().unwrap();
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
    order.emitter.on("OrderCreated", move |_: String| {
        tx.send(()).unwrap();
    });

    order.create("order-1".into(), "alice".into());
    order.emitter.emit_queued();

    rx.recv_timeout(Duration::from_secs(1))
        .expect("OrderCreated callback never fired");
}

// =============================================================================
// Guards stay in sync
// =============================================================================

#[test]
fn guards_stay_in_sync_between_digest_and_enqueue() {
    let mut order = Order::default();
    order.create("order-1".into(), "alice".into());
    order.confirm();
    order.confirm(); // second confirm blocked by guard

    assert_eq!(order.entity.version(), 2);
    assert_eq!(order.emitter.queued_len(), 2);
}

// =============================================================================
// Typed event enum exists
// =============================================================================

#[test]
fn typed_event_enum_exists() {
    let created = OrderEvent::OrderCreated {
        order_id: "o-1".into(),
        customer: "alice".into(),
    };
    assert_eq!(created.event_name(), "OrderCreated");
    assert_eq!(OrderEvent::OrderConfirmed.event_name(), "OrderConfirmed");
    assert_eq!(OrderEvent::OrderShipped.event_name(), "OrderShipped");
}

// =============================================================================
// Custom emitter field: enqueue(my_emitter)
// =============================================================================

#[test]
fn custom_emitter_field_enqueues() {
    let mut notifier = Notifier::default();
    notifier.send("n-1".into(), "Hello world".into());

    assert_eq!(notifier.entity.version(), 1);
    assert_eq!(notifier.my_emitter.queued_len(), 1);
}

#[test]
fn custom_emitter_field_emits() {
    let mut notifier = Notifier::default();

    let (tx, rx) = mpsc::channel();
    notifier
        .my_emitter
        .on("NotificationSent", move |_: String| {
            tx.send(()).unwrap();
        });

    notifier.send("n-1".into(), "Hello".into());
    notifier.my_emitter.emit_queued();

    rx.recv_timeout(Duration::from_secs(1))
        .expect("NotificationSent callback never fired");
}

#[test]
fn custom_emitter_replay_does_not_enqueue() {
    let repo = HashMapRepository::new().queued().aggregate::<Notifier>();

    let mut notifier = Notifier::default();
    notifier.send("n-1".into(), "Hello".into());
    notifier.my_emitter.emit_queued();
    repo.commit(&mut notifier).unwrap();

    let loaded = repo.get("n-1").unwrap().unwrap();
    assert_eq!(loaded.my_emitter.queued_len(), 0);
    assert_eq!(loaded.message, "Hello");
}

#[test]
fn custom_emitter_typed_enum() {
    let event = NotifierEvent::NotificationSent {
        id: "n-1".into(),
        message: "Hello".into(),
    };
    assert_eq!(event.event_name(), "NotificationSent");
}

#[test]
fn notifier_has_no_upcasters() {
    assert!(Notifier::upcasters().is_empty());
}
