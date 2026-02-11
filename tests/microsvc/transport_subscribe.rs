//! Bus transport tests — subscribe (pub/sub fan-out).

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde_json::json;
use sourced_rust::bus::{Event, InMemoryQueue, Publisher, Subscribable};
use sourced_rust::microsvc::{self, Service};
use sourced_rust::{CommitAggregate, GetAggregate, HashMapRepository, Queueable};

use crate::support::{Counter, CreateCounter};

#[test]
fn dispatches_from_pubsub() {
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

    let subscriber = queue.new_subscriber();
    let handle = microsvc::subscribe(service.clone(), subscriber, Duration::from_millis(10));

    queue
        .publish(Event::with_string_payload(
            "evt-1",
            "counter.create",
            r#"{"id":"c1"}"#,
        ))
        .unwrap();

    thread::sleep(Duration::from_millis(200));

    let counter: Counter = service.repo().get_aggregate("c1").unwrap().unwrap();
    assert_eq!(counter.value, 0);

    let stats = handle.stop();
    assert_eq!(stats.handled, 1);
}
