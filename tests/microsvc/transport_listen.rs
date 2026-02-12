//! Bus transport tests — listen (point-to-point queue consumption).
//!
//! Uses `Bus::from_queue` for all queue interactions, proving the Bus
//! abstraction works end-to-end with `microsvc::listen`.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde_json::json;
use sourced_rust::bus::{Bus, Event, InMemoryQueue};
use sourced_rust::microsvc::{self, Service, Session};
use sourced_rust::{AggregateBuilder, HashMapRepository, Queueable};

use crate::handlers;
use crate::handlers::Repo;
use crate::models::counter::Counter;

fn counter_service() -> Arc<Service<Repo>> {
    Arc::new(sourced_rust::register_handlers!(
        Service::new(HashMapRepository::new().queued().aggregate::<Counter>()),
        handlers::counter_create,
        handlers::counter_increment,
        handlers::whoami,
    ))
}

#[test]
fn dispatches_from_queue() {
    let bus = Bus::from_queue(InMemoryQueue::new());
    let service = counter_service();

    let handle = microsvc::listen(
        service.clone(),
        "counters",
        bus.subscriber().clone(),
        Duration::from_millis(10),
    );

    bus.send(
        "counters",
        Event::with_string_payload("cmd-1", "counter.create", r#"{"id":"c1"}"#),
    )
    .unwrap();

    thread::sleep(Duration::from_millis(200));

    bus.send(
        "counters",
        Event::with_string_payload("cmd-2", "counter.increment", r#"{"id":"c1","amount":10}"#),
    )
    .unwrap();

    thread::sleep(Duration::from_millis(200));

    let counter: Counter = service.repo().get("c1").unwrap().unwrap();
    assert_eq!(counter.value, 10);

    let stats = handle.stop();
    assert_eq!(stats.handled, 2);
    assert_eq!(stats.failed, 0);
}

#[test]
fn tracks_failures() {
    let bus = Bus::from_queue(InMemoryQueue::new());
    let service = counter_service();

    let handle = microsvc::listen(
        service.clone(),
        "counters",
        bus.subscriber().clone(),
        Duration::from_millis(10),
    );

    bus.send(
        "counters",
        Event::with_string_payload(
            "cmd-1",
            "counter.increment",
            r#"{"id":"nonexistent","amount":1}"#,
        ),
    )
    .unwrap();

    thread::sleep(Duration::from_millis(200));

    let stats = handle.stop();
    assert_eq!(stats.handled, 0);
    assert_eq!(stats.failed, 1);
}

#[test]
fn coexists_with_direct_dispatch() {
    let bus = Bus::from_queue(InMemoryQueue::new());
    let service = counter_service();

    let handle = microsvc::listen(
        service.clone(),
        "counters",
        bus.subscriber().clone(),
        Duration::from_millis(10),
    );

    // Create c1 via bus
    bus.send(
        "counters",
        Event::with_string_payload("cmd-1", "counter.create", r#"{"id":"c1"}"#),
    )
    .unwrap();

    thread::sleep(Duration::from_millis(200));

    // Create c2 via direct dispatch
    service
        .dispatch("counter.create", json!({ "id": "c2" }), Session::new())
        .unwrap();

    let c1: Counter = service.repo().get("c1").unwrap().unwrap();
    let c2: Counter = service.repo().get("c2").unwrap().unwrap();
    assert_eq!(c1.value, 0);
    assert_eq!(c2.value, 0);

    let stats = handle.stop();
    assert_eq!(stats.handled, 1);
}

#[test]
fn metadata_becomes_session() {
    let bus = Bus::from_queue(InMemoryQueue::new());
    let service = counter_service();

    let handle = microsvc::listen(
        service.clone(),
        "commands",
        bus.subscriber().clone(),
        Duration::from_millis(10),
    );

    let event = Event::with_string_payload("cmd-1", "whoami", "{}")
        .with_metadata("x-hasura-user-id", "user-42");
    bus.send("commands", event).unwrap();

    thread::sleep(Duration::from_millis(200));

    let stats = handle.stop();
    assert_eq!(stats.handled, 1);
    assert_eq!(stats.failed, 0);
}

#[test]
fn multiple_services_on_different_queues() {
    let bus = Bus::from_queue(InMemoryQueue::new());
    let store = HashMapRepository::new();

    let service_a = Arc::new(sourced_rust::register_handlers!(
        Service::new(store.clone().queued().aggregate::<Counter>()),
        handlers::counter_create,
    ));

    let service_b = Arc::new(sourced_rust::register_handlers!(
        Service::new(store.queued().aggregate::<Counter>()),
        handlers::counter_increment,
    ));

    let handle_a = microsvc::listen(
        service_a.clone(),
        "creates",
        bus.subscriber().clone(),
        Duration::from_millis(10),
    );
    let handle_b = microsvc::listen(
        service_b.clone(),
        "increments",
        bus.subscriber().clone(),
        Duration::from_millis(10),
    );

    bus.send(
        "creates",
        Event::with_string_payload("cmd-1", "counter.create", r#"{"id":"c1"}"#),
    )
    .unwrap();

    thread::sleep(Duration::from_millis(200));

    bus.send(
        "increments",
        Event::with_string_payload("cmd-2", "counter.increment", r#"{"id":"c1","amount":42}"#),
    )
    .unwrap();

    thread::sleep(Duration::from_millis(200));

    let counter: Counter = service_a.repo().get("c1").unwrap().unwrap();
    assert_eq!(counter.value, 42);

    let stats_a = handle_a.stop();
    let stats_b = handle_b.stop();
    assert_eq!(stats_a.handled, 1);
    assert_eq!(stats_b.handled, 1);
}
