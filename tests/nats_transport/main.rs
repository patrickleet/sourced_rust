//! NATS JetStream transport adapter integration tests.
//!
//! Publishes via `NatsPublisher` and consumes via `NatsJetStreamSource` against a
//! JetStream-enabled NATS server. Skips when `NATS_URL` is unset.
#![cfg(feature = "nats")]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use distributed::bus::{
    run_source, AsyncMessagePublisher, Bus, BusConsumer, NatsBus, NatsJetStreamSource,
    NatsPublisher, RunOptions,
};
use distributed::microsvc::{Context, Message, MessageKind, Service};
use serde_json::json;

// Shared broker-test helpers (recording_for, run_token, unique, ...).
#[path = "../transport_conformance/mod.rs"]
mod conformance;
use conformance::{
    named_recording_for as named_recording_service, recording_for as recording_service, unique,
};

fn nats_url() -> Option<String> {
    match std::env::var("NATS_URL") {
        Ok(url) => Some(url),
        Err(_) => {
            eprintln!("skipping nats transport test: NATS_URL is not set");
            None
        }
    }
}

#[tokio::test]
async fn publish_then_consume_round_trips_through_jetstream() {
    let Some(url) = nats_url() else { return };
    let subject = unique("order.initialized");
    let stream = unique("STREAM");
    let durable = unique("consumer");

    // Create the stream + durable consumer first so the stream exists before we
    // publish (JetStream publish requires a stream bound to the subject).
    let source = NatsJetStreamSource::connect(&url, &stream, vec![subject.clone()], &durable)
        .await
        .expect("connect source")
        .with_fetch_timeout(Duration::from_millis(800));

    // Publish three events.
    let publisher = NatsPublisher::connect(&url)
        .await
        .expect("connect publisher");
    for i in 0..3 {
        let message =
            Message::new(&subject, MessageKind::Event, b"{}".to_vec()).with_id(format!("m{i}"));
        publisher.publish(message).await.expect("publish");
    }

    // Consume via the shared runner.
    let handled = Arc::new(Mutex::new(Vec::<String>::new()));
    let h = handled.clone();
    let subject_for_handler = subject.clone();
    let service = Arc::new(
        Service::new()
            .event(Box::leak(subject.clone().into_boxed_str()))
            .handle(move |ctx: &Context<()>| {
                assert_eq!(ctx.message().name(), subject_for_handler);
                h.lock()
                    .unwrap()
                    .push(ctx.message().id().unwrap_or_default().to_string());
                async move { Ok(json!({})) }
            }),
    );

    run_source(service, source, RunOptions::idempotent())
        .await
        .expect("run_source drains the stream");

    let mut ids = handled.lock().unwrap().clone();
    ids.sort();
    assert_eq!(
        ids,
        vec!["m0".to_string(), "m1".to_string(), "m2".to_string()]
    );
}

#[tokio::test]
async fn message_id_and_metadata_survive_the_round_trip() {
    let Some(url) = nats_url() else { return };
    let subject = unique("order.initialized");
    let stream = unique("STREAM");
    let durable = unique("consumer");

    let source = NatsJetStreamSource::connect(&url, &stream, vec![subject.clone()], &durable)
        .await
        .expect("connect source")
        .with_fetch_timeout(Duration::from_millis(800));

    let publisher = NatsPublisher::connect(&url)
        .await
        .expect("connect publisher");
    let message = Message::new(&subject, MessageKind::Event, br#"{"k":"v"}"#.to_vec())
        .with_id("evt-1")
        .with_metadata("correlation_id", "corr-9");
    publisher.publish(message).await.expect("publish");

    let observed = Arc::new(Mutex::new(None));
    let o = observed.clone();
    let service = Arc::new(
        Service::new()
            .event(Box::leak(subject.clone().into_boxed_str()))
            .handle(move |ctx: &Context<()>| {
                let m = ctx.message();
                let recorded = Some((
                    m.id().map(str::to_string),
                    m.correlation_id().map(str::to_string),
                    m.payload().to_vec(),
                ));
                *o.lock().unwrap() = recorded;
                async move { Ok(json!({})) }
            }),
    );
    run_source(service, source, RunOptions::idempotent())
        .await
        .unwrap();

    let got = observed.lock().unwrap().clone().expect("handler ran");
    assert_eq!(got.0.as_deref(), Some("evt-1"));
    assert_eq!(got.1.as_deref(), Some("corr-9"));
    assert_eq!(got.2, br#"{"k":"v"}"#.to_vec());
}

/// `send` + `listen`: replicas sharing a `group` compete for the command — each
/// message is handled exactly once across the pool (point-to-point).
#[tokio::test]
async fn bus_send_listen_is_point_to_point_across_a_group() {
    let Some(url) = nats_url() else { return };
    let namespace = unique("ns").to_lowercase();
    let group = "orders";

    let producer = NatsBus::connect(&url)
        .group(group)
        .namespace(&namespace)
        .await
        .expect("connect producer")
        .with_fetch_timeout(Duration::from_millis(600));
    producer.ensure_stream().await.expect("ensure stream");

    let total = 6;
    for i in 0..total {
        producer
            .send_message(
                Message::new("order.initialize", MessageKind::Command, b"{}".to_vec())
                    .with_id(format!("c{i}")),
            )
            .await
            .expect("send command");
    }

    // Two replicas of the same service (same group) drain concurrently.
    let rec = Arc::new(Mutex::new(Vec::new()));
    let bus_a = NatsBus::connect(&url)
        .group(group)
        .namespace(&namespace)
        .await
        .unwrap()
        .with_fetch_timeout(Duration::from_millis(600));
    let bus_b = bus_a.clone();
    let svc_a = recording_service("order.initialize", MessageKind::Command, rec.clone());
    let svc_b = recording_service("order.initialize", MessageKind::Command, rec.clone());

    let (ra, rb) = tokio::join!(
        bus_a.listen(svc_a, RunOptions::idempotent()),
        bus_b.listen(svc_b, RunOptions::idempotent()),
    );
    ra.expect("replica a drains");
    rb.expect("replica b drains");

    let mut ids = rec.lock().unwrap().clone();
    ids.sort();
    let expected: Vec<String> = (0..total).map(|i| format!("c{i}")).collect();
    assert_eq!(
        ids, expected,
        "every command handled exactly once across the group"
    );
}

/// `publish` + `subscribe`: distinct `group`s each get their own durable on the
/// shared stream, so every group sees every event (fan-out).
#[tokio::test]
async fn bus_publish_subscribe_fans_out_across_groups() {
    let Some(url) = nats_url() else { return };
    let namespace = unique("ns").to_lowercase();

    let producer = NatsBus::connect(&url)
        .group("publisher")
        .namespace(&namespace)
        .await
        .expect("connect producer");
    producer.ensure_stream().await.expect("ensure stream");

    let total = 4;
    for i in 0..total {
        producer
            .publish_message(
                Message::new("order.initialized", MessageKind::Event, b"{}".to_vec())
                    .with_id(format!("e{i}")),
            )
            .await
            .expect("publish event");
    }

    let expected: Vec<String> = (0..total).map(|i| format!("e{i}")).collect();
    for group in ["projections", "audit"] {
        let bus = NatsBus::connect(&url)
            .group(group)
            .namespace(&namespace)
            .await
            .unwrap()
            .with_fetch_timeout(Duration::from_millis(600));
        let rec = Arc::new(Mutex::new(Vec::new()));
        bus.subscribe(
            recording_service("order.initialized", MessageKind::Event, rec.clone()),
            RunOptions::idempotent(),
        )
        .await
        .expect("subscriber drains");
        let mut ids = rec.lock().unwrap().clone();
        ids.sort();
        assert_eq!(ids, expected, "group {group} sees every event");
    }
}

#[tokio::test]
async fn bus_subscribe_uses_named_service_as_consumer_group() {
    let Some(url) = nats_url() else { return };
    let namespace = unique("ns").to_lowercase();

    let producer = NatsBus::connect(&url)
        .namespace(&namespace)
        .await
        .expect("connect producer");
    producer.ensure_stream().await.expect("ensure stream");

    for i in 0..3 {
        producer
            .publish_message(
                Message::new("order.initialized", MessageKind::Event, b"{}".to_vec())
                    .with_id(format!("e{i}")),
            )
            .await
            .expect("publish event");
    }

    let rec = Arc::new(Mutex::new(Vec::new()));
    NatsBus::connect(&url)
        .namespace(&namespace)
        .await
        .unwrap()
        .with_fetch_timeout(Duration::from_millis(600))
        .subscribe(
            named_recording_service(
                "order-projection",
                "order.initialized",
                MessageKind::Event,
                rec.clone(),
            ),
            RunOptions::idempotent(),
        )
        .await
        .expect("subscriber drains");

    let mut ids = rec.lock().unwrap().clone();
    ids.sort();
    assert_eq!(
        ids,
        vec!["e0".to_string(), "e1".to_string(), "e2".to_string()]
    );
}
