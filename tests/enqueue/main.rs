mod aggregate;

use sourced_rust::{AggregateBuilder, HashMapRepository, Queueable};
use std::sync::mpsc;
use std::time::Duration;

use aggregate::{Ephemeral, Notifier, Order};

// =============================================================================
// #[enqueue] basic behavior
// =============================================================================

#[test]
fn enqueue_queues_events_during_method_call() {
    let mut order = Order::default();
    order.create("order-1".into(), "alice".into());

    assert_eq!(order.emitter.queued_len(), 1);
}

#[test]
fn enqueue_queues_multiple_events_across_calls() {
    let mut order = Order::default();
    order.create("order-1".into(), "alice".into());
    order.confirm();

    assert_eq!(order.emitter.queued_len(), 2);
}

#[test]
fn emit_queued_fires_registered_listeners() {
    let mut order = Order::default();

    let (tx, rx) = mpsc::channel();
    order.emitter.on("OrderCreated", move |payload: String| {
        tx.send(payload).unwrap();
    });

    order.create("order-1".into(), "alice".into());
    order.emitter.emit_queued();

    let payload = rx.recv_timeout(Duration::from_secs(1)).expect("callback never fired");
    assert!(!payload.is_empty());
}

#[test]
fn emit_queued_drains_the_queue() {
    let mut order = Order::default();
    order.create("order-1".into(), "alice".into());

    assert_eq!(order.emitter.queued_len(), 1);
    order.emitter.emit_queued();
    assert_eq!(order.emitter.queued_len(), 0);
}

#[test]
fn emit_queued_fires_correct_event_types() {
    let mut order = Order::default();

    let (tx_created, rx_created) = mpsc::channel();
    order.emitter.on("OrderCreated", move |_: String| {
        tx_created.send("OrderCreated").unwrap();
    });

    let (tx_confirmed, rx_confirmed) = mpsc::channel();
    order.emitter.on("OrderConfirmed", move |_: String| {
        tx_confirmed.send("OrderConfirmed").unwrap();
    });

    order.create("order-1".into(), "alice".into());
    order.confirm();
    order.emitter.emit_queued();

    rx_created
        .recv_timeout(Duration::from_secs(1))
        .expect("OrderCreated callback never fired");
    rx_confirmed
        .recv_timeout(Duration::from_secs(1))
        .expect("OrderConfirmed callback never fired");
}

// =============================================================================
// #[enqueue] guard conditions
// =============================================================================

#[test]
fn enqueue_guard_prevents_event_when_condition_false() {
    let mut order = Order::default();
    order.create("order-1".into(), "alice".into());

    // Try to ship without confirming first — guard blocks it
    order.ship();

    // Only OrderCreated should be queued, not OrderShipped
    assert_eq!(order.emitter.queued_len(), 1);
    assert_eq!(order.status, "created");
}

#[test]
fn enqueue_guard_allows_event_when_condition_true() {
    let mut order = Order::default();
    order.create("order-1".into(), "alice".into());
    order.confirm();
    order.ship();

    assert_eq!(order.emitter.queued_len(), 3);
    assert_eq!(order.status, "shipped");
}

#[test]
fn enqueue_guard_on_empty_value() {
    let mut eph = Ephemeral::default();

    // value is empty, guard blocks clear
    eph.clear();
    assert_eq!(eph.emitter.queued_len(), 0);

    // set a value, now clear should work
    eph.set_value("hello".into());
    eph.clear();
    assert_eq!(eph.emitter.queued_len(), 2); // ValueSet + ValueCleared
}

// =============================================================================
// #[enqueue] with custom emitter field
// =============================================================================

#[test]
fn enqueue_with_custom_field_name() {
    let mut notifier = Notifier::default();
    notifier.send("n-1".into(), "Hello world".into());

    assert_eq!(notifier.my_emitter.queued_len(), 1);
}

#[test]
fn custom_field_emit_fires_listener() {
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

// =============================================================================
// #[digest] + #[enqueue] together
// =============================================================================

#[test]
fn digest_and_enqueue_both_record() {
    let mut order = Order::default();
    order.create("order-1".into(), "alice".into());

    // digest records to entity event stream
    assert_eq!(order.entity.version(), 1);

    // enqueue queues for local emission
    assert_eq!(order.emitter.queued_len(), 1);
}

#[test]
fn digest_and_enqueue_full_lifecycle() {
    let mut order = Order::default();
    order.create("order-1".into(), "alice".into());
    order.confirm();
    order.ship();

    // 3 events digested
    assert_eq!(order.entity.version(), 3);

    // 3 events queued
    assert_eq!(order.emitter.queued_len(), 3);

    assert_eq!(order.status, "shipped");
}

#[test]
fn digest_and_enqueue_guards_stay_in_sync() {
    let mut order = Order::default();
    order.create("order-1".into(), "alice".into());

    // Try to confirm twice — guard blocks second call for both digest and enqueue
    order.confirm();
    order.confirm();

    assert_eq!(order.entity.version(), 2); // OrderCreated + OrderConfirmed
    assert_eq!(order.emitter.queued_len(), 2);
}

// =============================================================================
// #[enqueue] with repository commit + replay
// =============================================================================

#[test]
fn enqueue_events_survive_commit_and_emit_after() {
    let repo = HashMapRepository::new().queued().aggregate::<Order>();

    let mut order = Order::default();
    order.create("order-1".into(), "alice".into());
    order.confirm();

    // Events queued before commit
    assert_eq!(order.emitter.queued_len(), 2);

    repo.commit(&mut order).unwrap();

    // Events still queued after commit — emit is explicit
    assert_eq!(order.emitter.queued_len(), 2);

    let (tx, rx) = mpsc::channel();
    order.emitter.on("OrderCreated", move |_: String| {
        tx.send(()).unwrap();
    });

    order.emitter.emit_queued();

    rx.recv_timeout(Duration::from_secs(1))
        .expect("OrderCreated callback never fired after commit");
    assert_eq!(order.emitter.queued_len(), 0);
}

#[test]
fn replay_does_not_enqueue_events() {
    let repo = HashMapRepository::new().queued().aggregate::<Order>();

    let mut order = Order::default();
    order.create("order-1".into(), "alice".into());
    order.confirm();
    order.emitter.emit_queued();

    repo.commit(&mut order).unwrap();

    // Load from repo — replays digest events but should NOT re-enqueue
    let loaded = repo.get("order-1").unwrap().unwrap();
    assert_eq!(loaded.emitter.queued_len(), 0);
    assert_eq!(loaded.status, "confirmed");
    assert_eq!(loaded.entity.version(), 2);
}

// =============================================================================
// #[enqueue] only (no digest)
// =============================================================================

#[test]
fn enqueue_only_without_digest() {
    let mut eph = Ephemeral::default();
    eph.set_value("test".into());

    // Queued for emission
    assert_eq!(eph.emitter.queued_len(), 1);

    // No events digested to entity
    assert_eq!(eph.entity.version(), 0);
}

#[test]
fn enqueue_only_emits_correctly() {
    let mut eph = Ephemeral::default();

    let (tx, rx) = mpsc::channel();
    eph.emitter.on("ValueSet", move |payload: String| {
        tx.send(payload).unwrap();
    });

    eph.set_value("hello".into());
    eph.emitter.emit_queued();

    let payload = rx.recv_timeout(Duration::from_secs(1)).expect("ValueSet callback never fired");
    assert!(!payload.is_empty());
}
