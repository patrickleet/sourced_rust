//! RabbitMQ transport adapter integration tests.
//!
//! Publishes via `RabbitPublisher` (publisher confirms) and consumes via
//! `RabbitSource` (`basic_get`) against a broker. Skips when `AMQP_URL` is unset.
#![cfg(feature = "rabbitmq")]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use distributed::bus::{
    run_source, AsyncMessagePublisher, Bus, BusConsumer, RabbitBus, RabbitPublisher, RabbitSource,
    RunOptions,
};
use distributed::microsvc::{Context, Message, MessageKind, Service};
use serde_json::json;

static SEQ: AtomicU64 = AtomicU64::new(1);

/// A token unique to this process run, so durable queue/exchange names don't
/// collide with state left by a previous run against the same broker.
fn run_token() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
        ^ u128::from(std::process::id())
}

fn amqp_url() -> Option<String> {
    match std::env::var("AMQP_URL") {
        Ok(url) => Some(url),
        Err(_) => {
            eprintln!("skipping rabbitmq transport test: AMQP_URL is not set");
            None
        }
    }
}

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}_{:x}_{}",
        run_token(),
        SEQ.fetch_add(1, Ordering::SeqCst)
    )
}

/// Service whose single handler records the message id; `kind` picks command vs
/// event registration.
fn recording_for(name: &str, kind: MessageKind, rec: Arc<Mutex<Vec<String>>>) -> Arc<Service<()>> {
    let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
    let builder = Service::new();
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

fn named_recording_for(
    service_name: &str,
    name: &str,
    kind: MessageKind,
    rec: Arc<Mutex<Vec<String>>>,
) -> Arc<Service<()>> {
    let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
    let builder = Service::new().named(service_name.to_string());
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

#[tokio::test]
async fn publish_then_consume_round_trips_through_rabbitmq() {
    let Some(url) = amqp_url() else { return };
    let queue = unique("order.initialized");

    // Declare the queue (source) before publishing so the default-exchange route
    // has a destination.
    let source = RabbitSource::connect(&url, &queue)
        .await
        .expect("connect source");
    let publisher = RabbitPublisher::connect(&url)
        .await
        .expect("connect publisher");
    for i in 0..3 {
        let message =
            Message::new(&queue, MessageKind::Event, b"{}".to_vec()).with_id(format!("m{i}"));
        publisher.publish(message).await.expect("publish");
    }

    let handled = Arc::new(Mutex::new(Vec::<String>::new()));
    let h = handled.clone();
    let service = Arc::new(
        Service::new()
            .event(Box::leak(queue.clone().into_boxed_str()))
            .handle(move |ctx: &Context<()>| {
                h.lock()
                    .unwrap()
                    .push(ctx.message().id().unwrap_or_default().to_string());
                async move { Ok(json!({})) }
            }),
    );
    run_source(service, source, RunOptions::idempotent())
        .await
        .expect("run_source drains the queue");

    let mut ids = handled.lock().unwrap().clone();
    ids.sort();
    assert_eq!(
        ids,
        vec!["m0".to_string(), "m1".to_string(), "m2".to_string()]
    );
}

#[tokio::test]
async fn message_id_and_metadata_survive_the_round_trip() {
    let Some(url) = amqp_url() else { return };
    let queue = unique("order.initialized");

    let source = RabbitSource::connect(&url, &queue)
        .await
        .expect("connect source");
    let publisher = RabbitPublisher::connect(&url)
        .await
        .expect("connect publisher");
    // Use a non-default content type to prove it survives the AMQP round-trip
    // (Message::new defaults to application/json).
    let mut message = Message::new(&queue, MessageKind::Event, br#"{"k":"v"}"#.to_vec())
        .with_id("evt-1")
        .with_metadata("correlation_id", "corr-9");
    message.content_type = "application/cloudevents+json".to_string();
    publisher.publish(message).await.expect("publish");

    let observed = Arc::new(Mutex::new(None));
    let o = observed.clone();
    let service = Arc::new(
        Service::new()
            .event(Box::leak(queue.clone().into_boxed_str()))
            .handle(move |ctx: &Context<()>| {
                let m = ctx.message();
                let recorded = Some((
                    m.id().map(str::to_string),
                    m.correlation_id().map(str::to_string),
                    m.payload().to_vec(),
                    m.content_type.clone(),
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
    assert_eq!(
        got.3, "application/cloudevents+json",
        "content_type round-trips"
    );
}

// ---- RabbitBus: send/listen (default exchange) + publish/subscribe (topic exchange) ----

/// `send` + `listen`: a durable command queue is shared by replicas of a group,
/// so AMQP round-robins — each command handled exactly once (point-to-point).
#[tokio::test]
async fn bus_send_listen_is_point_to_point_across_a_group() {
    let Some(url) = amqp_url() else { return };
    let ns = unique("ns").to_lowercase();

    let producer = RabbitBus::connect(&url)
        .group("orders")
        .namespace(&ns)
        .await
        .expect("connect producer");
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

    // Two replicas of the same group (separate connections) drain concurrently.
    let rec = Arc::new(Mutex::new(Vec::new()));
    let bus_a = RabbitBus::connect(&url)
        .group("orders")
        .namespace(&ns)
        .await
        .unwrap();
    let bus_b = RabbitBus::connect(&url)
        .group("orders")
        .namespace(&ns)
        .await
        .unwrap();
    let (ra, rb) = tokio::join!(
        bus_a.listen(
            recording_for("order.initialize", MessageKind::Command, rec.clone()),
            RunOptions::idempotent()
        ),
        bus_b.listen(
            recording_for("order.initialize", MessageKind::Command, rec.clone()),
            RunOptions::idempotent()
        ),
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

/// `publish` + `subscribe`: each group binds its own queue to the topic exchange,
/// so every group receives every event (fan-out).
#[tokio::test]
async fn bus_publish_subscribe_fans_out_across_groups() {
    let Some(url) = amqp_url() else { return };
    let ns = unique("ns").to_lowercase();

    let producer = RabbitBus::connect(&url)
        .group("producer")
        .namespace(&ns)
        .await
        .expect("connect producer");

    // Bind both subscribers' queues BEFORE publishing — the topic exchange drops
    // events with no matching binding.
    let proj_rec = Arc::new(Mutex::new(Vec::new()));
    let audit_rec = Arc::new(Mutex::new(Vec::new()));
    let svc_proj = recording_for("order.initialized", MessageKind::Event, proj_rec.clone());
    let svc_audit = recording_for("order.initialized", MessageKind::Event, audit_rec.clone());
    let bus_proj = RabbitBus::connect(&url)
        .group("projections")
        .namespace(&ns)
        .await
        .unwrap();
    let bus_audit = RabbitBus::connect(&url)
        .group("audit")
        .namespace(&ns)
        .await
        .unwrap();
    bus_proj
        .ensure_subscription(svc_proj.as_ref())
        .await
        .expect("bind proj");
    bus_audit
        .ensure_subscription(svc_audit.as_ref())
        .await
        .expect("bind audit");

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

    bus_proj
        .subscribe(svc_proj, RunOptions::idempotent())
        .await
        .expect("proj drains");
    bus_audit
        .subscribe(svc_audit, RunOptions::idempotent())
        .await
        .expect("audit drains");

    let expected: Vec<String> = (0..total).map(|i| format!("e{i}")).collect();
    let mut proj_ids = proj_rec.lock().unwrap().clone();
    let mut audit_ids = audit_rec.lock().unwrap().clone();
    proj_ids.sort();
    audit_ids.sort();
    assert_eq!(proj_ids, expected, "projections sees every event");
    assert_eq!(audit_ids, expected, "audit sees every event");
}

#[tokio::test]
async fn bus_subscribe_uses_named_service_as_consumer_group() {
    let Some(url) = amqp_url() else { return };
    let ns = unique("ns").to_lowercase();

    let producer = RabbitBus::connect(&url)
        .namespace(&ns)
        .await
        .expect("connect producer");
    let bus = RabbitBus::connect(&url)
        .namespace(&ns)
        .await
        .expect("connect subscriber");
    let rec = Arc::new(Mutex::new(Vec::new()));
    let service = named_recording_for(
        "order-projection",
        "order.initialized",
        MessageKind::Event,
        rec.clone(),
    );
    bus.ensure_subscription(service.as_ref())
        .await
        .expect("bind subscriber");

    for i in 0..3 {
        producer
            .publish_message(
                Message::new("order.initialized", MessageKind::Event, b"{}".to_vec())
                    .with_id(format!("e{i}")),
            )
            .await
            .expect("publish event");
    }

    bus.subscribe(service, RunOptions::idempotent())
        .await
        .expect("subscriber drains");

    let mut ids = rec.lock().unwrap().clone();
    ids.sort();
    assert_eq!(
        ids,
        vec!["e0".to_string(), "e1".to_string(), "e2".to_string()]
    );
}
