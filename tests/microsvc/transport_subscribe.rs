//! Bus transport tests — subscribe (pub/sub fan-out).
//!
//! Uses `Bus::from_queue` for all event interactions, proving the Bus
//! abstraction works end-to-end with `microsvc::subscribe`.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use sourced_rust::bus::{Bus, Event, InMemoryQueue, Subscribable};
use sourced_rust::microsvc::{self, Service};
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
fn dispatches_from_pubsub() {
    let bus = Bus::from_queue(InMemoryQueue::new());
    let service = counter_service();

    let subscriber = bus.subscriber().new_subscriber();
    let handle = microsvc::subscribe(service.clone(), subscriber, Duration::from_millis(10));

    // Create
    bus.publish(Event::with_string_payload(
        "evt-1",
        "counter.create",
        r#"{"id":"c1"}"#,
    ))
    .unwrap();

    thread::sleep(Duration::from_millis(200));

    // Increment
    bus.publish(Event::with_string_payload(
        "evt-2",
        "counter.increment",
        r#"{"id":"c1","amount":10}"#,
    ))
    .unwrap();

    thread::sleep(Duration::from_millis(200));

    // Increment again
    bus.publish(Event::with_string_payload(
        "evt-3",
        "counter.increment",
        r#"{"id":"c1","amount":5}"#,
    ))
    .unwrap();

    thread::sleep(Duration::from_millis(200));

    // Stop the worker before reading to avoid lock contention
    let stats = handle.stop();
    assert_eq!(stats.failed, 0);
    assert_eq!(stats.handled, 3);

    let counter: Counter = service.repo().get("c1").unwrap().unwrap();
    assert_eq!(counter.value, 15);
}
