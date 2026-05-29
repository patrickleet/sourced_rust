//! NATS JetStream transport adapter integration tests.
//!
//! Publishes via `NatsPublisher` and consumes via `NatsJetStreamSource` against a
//! JetStream-enabled NATS server. Skips when `NATS_URL` is unset.
#![cfg(feature = "nats")]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use sourced_rust::microsvc::transport::{
    run_source, AsyncMessagePublisher, NatsJetStreamSource, NatsPublisher, RunOptions,
};
use sourced_rust::microsvc::{Message, MessageKind, Service};

static SEQ: AtomicU64 = AtomicU64::new(1);

fn nats_url() -> Option<String> {
    match std::env::var("NATS_URL") {
        Ok(url) => Some(url),
        Err(_) => {
            eprintln!("skipping nats transport test: NATS_URL is not set");
            None
        }
    }
}

/// Unique subject/stream/durable per test so JetStream state does not collide.
fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", SEQ.fetch_add(1, Ordering::SeqCst))
}

#[tokio::test]
async fn publish_then_consume_round_trips_through_jetstream() {
    let Some(url) = nats_url() else { return };
    let subject = unique("evt");
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
        Service::new(())
            .event(Box::leak(subject.clone().into_boxed_str()))
            .handle(move |ctx| {
                assert_eq!(ctx.message().name(), subject_for_handler);
                h.lock()
                    .unwrap()
                    .push(ctx.message().id().unwrap_or_default().to_string());
                Ok(json!({}))
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
    let subject = unique("evt");
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
        Service::new(())
            .event(Box::leak(subject.clone().into_boxed_str()))
            .handle(move |ctx| {
                let m = ctx.message();
                *o.lock().unwrap() = Some((
                    m.id().map(str::to_string),
                    m.correlation_id().map(str::to_string),
                    m.payload().to_vec(),
                ));
                Ok(json!({}))
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
