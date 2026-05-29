//! Kafka transport adapter integration tests.
//!
//! Publishes via `KafkaPublisher` (acks=all) and consumes via `KafkaSource`
//! (consumer group, offset commit on ack) against a broker. Skips when
//! `KAFKA_BROKERS` is unset.
#![cfg(feature = "kafka")]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use sourced_rust::microsvc::transport::{
    run_source, AsyncMessagePublisher, KafkaPublisher, KafkaSource, RunOptions,
};
use sourced_rust::microsvc::{Message, MessageKind, Service};

static SEQ: AtomicU64 = AtomicU64::new(1);

fn brokers() -> Option<String> {
    match std::env::var("KAFKA_BROKERS") {
        Ok(b) => Some(b),
        Err(_) => {
            eprintln!("skipping kafka transport test: KAFKA_BROKERS is not set");
            None
        }
    }
}

fn unique(prefix: &str) -> String {
    // Kafka persists topics across runs, so include a per-process time component
    // to avoid reading stale messages from a previous run's same-named topic.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{prefix}-{nanos}-{}", SEQ.fetch_add(1, Ordering::SeqCst))
}

#[tokio::test]
async fn publish_then_consume_round_trips_through_kafka() {
    let Some(brokers) = brokers() else { return };
    let topic = unique("evt");
    let group = unique("group");

    // Produce first so the topic auto-creates; then a fresh group reads from
    // earliest.
    let publisher = KafkaPublisher::connect(&brokers).await.expect("producer");
    for i in 0..3 {
        let message =
            Message::new(&topic, MessageKind::Event, b"{}".to_vec()).with_id(format!("m{i}"));
        publisher.publish(message).await.expect("publish");
    }

    let source = KafkaSource::connect(&brokers, &group, &[&topic])
        .await
        .expect("consumer")
        .with_fetch_timeout(Duration::from_secs(10));

    let handled = Arc::new(Mutex::new(Vec::<String>::new()));
    let h = handled.clone();
    let service = Arc::new(
        Service::new(())
            .event(Box::leak(topic.clone().into_boxed_str()))
            .handle(move |ctx| {
                h.lock()
                    .unwrap()
                    .push(ctx.message().id().unwrap_or_default().to_string());
                Ok(json!({}))
            }),
    );
    run_source(service, source, RunOptions::idempotent())
        .await
        .expect("run_source drains the topic");

    let mut ids = handled.lock().unwrap().clone();
    ids.sort();
    assert_eq!(
        ids,
        vec!["m0".to_string(), "m1".to_string(), "m2".to_string()]
    );
}

#[tokio::test]
async fn message_id_and_metadata_survive_the_round_trip() {
    let Some(brokers) = brokers() else { return };
    let topic = unique("evt");
    let group = unique("group");

    let publisher = KafkaPublisher::connect(&brokers).await.expect("producer");
    publisher
        .publish(
            Message::new(&topic, MessageKind::Event, br#"{"k":"v"}"#.to_vec())
                .with_id("evt-1")
                .with_metadata("correlation_id", "corr-9"),
        )
        .await
        .expect("publish");

    let source = KafkaSource::connect(&brokers, &group, &[&topic])
        .await
        .expect("consumer")
        .with_fetch_timeout(Duration::from_secs(10));

    let observed = Arc::new(Mutex::new(None));
    let o = observed.clone();
    let service = Arc::new(
        Service::new(())
            .event(Box::leak(topic.clone().into_boxed_str()))
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
