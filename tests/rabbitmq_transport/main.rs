//! RabbitMQ transport adapter integration tests.
//!
//! Publishes via `RabbitPublisher` (publisher confirms) and consumes via
//! `RabbitSource` (`basic_get`) against a broker. Skips when `AMQP_URL` is unset.
#![cfg(feature = "rabbitmq")]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::json;
use sourced_rust::microsvc::transport::{
    run_source, AsyncMessagePublisher, RabbitPublisher, RabbitSource, RunOptions,
};
use sourced_rust::microsvc::{Message, MessageKind, Service};

static SEQ: AtomicU64 = AtomicU64::new(1);

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
    format!("{prefix}_{}", SEQ.fetch_add(1, Ordering::SeqCst))
}

#[tokio::test]
async fn publish_then_consume_round_trips_through_rabbitmq() {
    let Some(url) = amqp_url() else { return };
    let queue = unique("evt");

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
        Service::new(())
            .event(Box::leak(queue.clone().into_boxed_str()))
            .handle(move |ctx| {
                h.lock()
                    .unwrap()
                    .push(ctx.message().id().unwrap_or_default().to_string());
                Ok(json!({}))
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
    let queue = unique("evt");

    let source = RabbitSource::connect(&url, &queue)
        .await
        .expect("connect source");
    let publisher = RabbitPublisher::connect(&url)
        .await
        .expect("connect publisher");
    publisher
        .publish(
            Message::new(&queue, MessageKind::Event, br#"{"k":"v"}"#.to_vec())
                .with_id("evt-1")
                .with_metadata("correlation_id", "corr-9"),
        )
        .await
        .expect("publish");

    let observed = Arc::new(Mutex::new(None));
    let o = observed.clone();
    let service = Arc::new(
        Service::new(())
            .event(Box::leak(queue.clone().into_boxed_str()))
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
