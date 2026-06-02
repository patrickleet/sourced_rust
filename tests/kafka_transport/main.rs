//! Kafka transport adapter integration tests.
//!
//! Publishes via `KafkaPublisher` (acks=all) and consumes via `KafkaSource`
//! (consumer group, offset commit on ack) against a broker. Skips when
//! `KAFKA_BROKERS` is unset.
#![cfg(feature = "kafka")]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use distributed::bus::{
    run_source, AsyncMessagePublisher, Bus, BusConsumer, KafkaBus, KafkaPublisher, KafkaSource,
    RunOptions,
};
use distributed::microsvc::{Context, Message, MessageKind, Service};
use serde_json::json;

static SEQ: AtomicU64 = AtomicU64::new(1);

/// Service whose single handler records the message id; `kind` picks command vs
/// event registration.
fn recording_for(name: &str, kind: MessageKind, rec: Arc<Mutex<Vec<String>>>) -> Arc<Service<()>> {
    let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
    let builder = Service::new(());
    let registered = match kind {
        MessageKind::Command => builder.command(leaked),
        MessageKind::Event => builder.event(leaked),
    };
    Arc::new(registered.handle(move |ctx: &Context<()>| {
        rec.lock()
            .unwrap()
            .push(ctx.message().id().unwrap_or_default().to_string());
        async move { Ok(json!({})) }
    }))
}

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
    let topic = unique("order.initialized");
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
            .handle(move |ctx: &Context<()>| {
                h.lock()
                    .unwrap()
                    .push(ctx.message().id().unwrap_or_default().to_string());
                async move { Ok(json!({})) }
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
    let topic = unique("order.initialized");
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

// ---- KafkaBus: shared group = listen (point-to-point); group-per-service = subscribe (fan-out) ----

/// `send` + `listen`: a shared consumer group consumes each command once for the
/// group as a whole. Proven deterministically: a first listener drains every
/// command; a second listener in the **same group** then reads nothing, because
/// the group's offset is already committed past the end — point-to-point.
#[tokio::test]
async fn bus_listen_shared_group_consumes_each_command_once() {
    let Some(brokers) = brokers() else { return };
    let ns = unique("ns");

    let producer = KafkaBus::connect(&brokers, "orders", &ns)
        .await
        .expect("connect producer");
    let total = 5;
    for i in 0..total {
        producer
            .send_message(
                Message::new("order.initialize", MessageKind::Command, b"{}".to_vec())
                    .with_id(format!("c{i}")),
            )
            .await
            .expect("send command");
    }

    // First member of group "orders" drains every command.
    let first = Arc::new(Mutex::new(Vec::new()));
    KafkaBus::connect(&brokers, "orders", &ns)
        .await
        .unwrap()
        .with_fetch_timeout(Duration::from_secs(10))
        .listen(
            recording_for("order.initialize", MessageKind::Command, first.clone()),
            RunOptions::idempotent(),
        )
        .await
        .expect("first listener drains");
    let mut ids = first.lock().unwrap().clone();
    ids.sort();
    let expected: Vec<String> = (0..total).map(|i| format!("c{i}")).collect();
    assert_eq!(ids, expected, "the group consumes every command");

    // A second member of the SAME group sees nothing — the group already consumed
    // and committed past these records (point-to-point, not fan-out).
    let second = Arc::new(Mutex::new(Vec::new()));
    KafkaBus::connect(&brokers, "orders", &ns)
        .await
        .unwrap()
        .with_fetch_timeout(Duration::from_secs(6))
        .listen(
            recording_for("order.initialize", MessageKind::Command, second.clone()),
            RunOptions::idempotent(),
        )
        .await
        .expect("second listener drains");
    assert!(
        second.lock().unwrap().is_empty(),
        "a second consumer in the same group re-consumes nothing"
    );
}

/// `publish` + `subscribe`: each `group` is a distinct Kafka consumer group, and
/// Kafka delivers every record to every group — so each group reads every event
/// (fan-out). A fresh group reads from earliest.
#[tokio::test]
async fn bus_subscribe_fans_out_across_groups() {
    let Some(brokers) = brokers() else { return };
    let ns = unique("ns");

    let producer = KafkaBus::connect(&brokers, "producer", &ns)
        .await
        .expect("connect producer");
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
        let rec = Arc::new(Mutex::new(Vec::new()));
        KafkaBus::connect(&brokers, group, &ns)
            .await
            .unwrap()
            .with_fetch_timeout(Duration::from_secs(10))
            .subscribe(
                recording_for("order.initialized", MessageKind::Event, rec.clone()),
                RunOptions::idempotent(),
            )
            .await
            .expect("subscriber drains");
        let mut ids = rec.lock().unwrap().clone();
        ids.sort();
        assert_eq!(ids, expected, "group {group} sees every event");
    }
}
