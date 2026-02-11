//! Bus transport tests — listen (point-to-point queue consumption).

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde_json::json;
use sourced_rust::bus::{Event, InMemoryQueue, Sender};
use sourced_rust::microsvc::{self, HandlerError, Service, Session};
use sourced_rust::{CommitAggregate, GetAggregate, HashMapRepository, Queueable};

use crate::support::{Counter, CreateCounter, IncrementCounter};

#[test]
fn dispatches_from_queue() {
    let queue = InMemoryQueue::new();
    let service = Arc::new(
        Service::new(HashMapRepository::new().queued())
            .command("counter.create", |ctx| {
                let input = ctx.input::<CreateCounter>()?;
                let mut counter = Counter::default();
                counter.create(input.id.clone());
                ctx.repo().commit_aggregate(&mut counter)?;
                Ok(json!({ "id": input.id }))
            })
            .command("counter.increment", |ctx| {
                let input = ctx.input::<IncrementCounter>()?;
                let mut counter: Counter = ctx
                    .repo()
                    .get_aggregate(&input.id)?
                    .ok_or_else(|| HandlerError::NotFound(input.id.clone()))?;
                counter.increment(input.amount);
                ctx.repo().commit_aggregate(&mut counter)?;
                Ok(json!({ "value": counter.value }))
            }),
    );

    let handle = microsvc::listen(
        service.clone(),
        "counters",
        queue.clone(),
        Duration::from_millis(10),
    );

    queue
        .send(
            "counters",
            Event::with_string_payload("cmd-1", "counter.create", r#"{"id":"c1"}"#),
        )
        .unwrap();

    thread::sleep(Duration::from_millis(200));

    queue
        .send(
            "counters",
            Event::with_string_payload(
                "cmd-2",
                "counter.increment",
                r#"{"id":"c1","amount":10}"#,
            ),
        )
        .unwrap();

    thread::sleep(Duration::from_millis(200));

    let counter: Counter = service.repo().get_aggregate("c1").unwrap().unwrap();
    assert_eq!(counter.value, 10);

    let stats = handle.stop();
    assert_eq!(stats.handled, 2);
    assert_eq!(stats.failed, 0);
}

#[test]
fn tracks_failures() {
    let queue = InMemoryQueue::new();
    let service = Arc::new(
        Service::new(HashMapRepository::new().queued()).command("counter.increment", |ctx| {
            let input = ctx.input::<IncrementCounter>()?;
            let _counter: Counter = ctx
                .repo()
                .get_aggregate(&input.id)?
                .ok_or_else(|| HandlerError::NotFound(input.id.clone()))?;
            Ok(json!({}))
        }),
    );

    let handle = microsvc::listen(
        service.clone(),
        "counters",
        queue.clone(),
        Duration::from_millis(10),
    );

    queue
        .send(
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
    let queue = InMemoryQueue::new();
    let service = Arc::new(
        Service::new(HashMapRepository::new().queued()).command("counter.create", |ctx| {
            let input = ctx.input::<CreateCounter>()?;
            let mut counter = Counter::default();
            counter.create(input.id.clone());
            ctx.repo().commit_aggregate(&mut counter)?;
            Ok(json!({ "id": input.id }))
        }),
    );

    let handle = microsvc::listen(
        service.clone(),
        "counters",
        queue.clone(),
        Duration::from_millis(10),
    );

    // Create c1 via bus
    queue
        .send(
            "counters",
            Event::with_string_payload("cmd-1", "counter.create", r#"{"id":"c1"}"#),
        )
        .unwrap();

    thread::sleep(Duration::from_millis(200));

    // Create c2 via direct dispatch
    service
        .dispatch("counter.create", json!({ "id": "c2" }), Session::new())
        .unwrap();

    let c1: Counter = service.repo().get_aggregate("c1").unwrap().unwrap();
    let c2: Counter = service.repo().get_aggregate("c2").unwrap().unwrap();
    assert_eq!(c1.value, 0);
    assert_eq!(c2.value, 0);

    let stats = handle.stop();
    assert_eq!(stats.handled, 1);
}

#[test]
fn metadata_becomes_session() {
    let queue = InMemoryQueue::new();
    let service = Arc::new(
        Service::new(HashMapRepository::new().queued()).command("whoami", |ctx| {
            let user_id = ctx.user_id()?;
            Ok(json!({ "user_id": user_id }))
        }),
    );

    let handle = microsvc::listen(
        service.clone(),
        "commands",
        queue.clone(),
        Duration::from_millis(10),
    );

    let event = Event::with_string_payload("cmd-1", "whoami", "{}")
        .with_metadata("x-hasura-user-id", "user-42");
    queue.send("commands", event).unwrap();

    thread::sleep(Duration::from_millis(200));

    let stats = handle.stop();
    assert_eq!(stats.handled, 1);
    assert_eq!(stats.failed, 0);
}

#[test]
fn multiple_services_on_different_queues() {
    let queue = InMemoryQueue::new();
    let store = HashMapRepository::new();

    let service_a = Arc::new(
        Service::new(store.clone().queued()).command("counter.create", |ctx| {
            let input = ctx.input::<CreateCounter>()?;
            let mut counter = Counter::default();
            counter.create(input.id.clone());
            ctx.repo().commit_aggregate(&mut counter)?;
            Ok(json!({ "id": input.id }))
        }),
    );

    let service_b = Arc::new(
        Service::new(store.queued()).command("counter.increment", |ctx| {
            let input = ctx.input::<IncrementCounter>()?;
            let mut counter: Counter = ctx
                .repo()
                .get_aggregate(&input.id)?
                .ok_or_else(|| HandlerError::NotFound(input.id.clone()))?;
            counter.increment(input.amount);
            ctx.repo().commit_aggregate(&mut counter)?;
            Ok(json!({ "value": counter.value }))
        }),
    );

    let handle_a = microsvc::listen(
        service_a.clone(),
        "creates",
        queue.clone(),
        Duration::from_millis(10),
    );
    let handle_b = microsvc::listen(
        service_b.clone(),
        "increments",
        queue.clone(),
        Duration::from_millis(10),
    );

    queue
        .send(
            "creates",
            Event::with_string_payload("cmd-1", "counter.create", r#"{"id":"c1"}"#),
        )
        .unwrap();

    thread::sleep(Duration::from_millis(200));

    queue
        .send(
            "increments",
            Event::with_string_payload(
                "cmd-2",
                "counter.increment",
                r#"{"id":"c1","amount":42}"#,
            ),
        )
        .unwrap();

    thread::sleep(Duration::from_millis(200));

    let counter: Counter = service_a.repo().get_aggregate("c1").unwrap().unwrap();
    assert_eq!(counter.value, 42);

    let stats_a = handle_a.stop();
    let stats_b = handle_b.stop();
    assert_eq!(stats_a.handled, 1);
    assert_eq!(stats_b.handled, 1);
}
