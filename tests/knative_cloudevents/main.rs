//! Knative / CloudEvents HTTP ingress integration tests.
//!
//! Drives `cloud_events_router` over a real ephemeral HTTP server and asserts
//! the CloudEvents binding (binary + structured) and the ack/retry/permanent
//! response-status mapping. Runs in-process — no external broker.
#![cfg(feature = "http")]

use std::sync::{Arc, Mutex};

use serde_json::json;
use sourced_rust::microsvc::transport::cloud_events_router;
use sourced_rust::microsvc::{HandlerError, Service};

async fn spawn_server() -> (String, Arc<Mutex<Vec<String>>>) {
    let handled = Arc::new(Mutex::new(Vec::<String>::new()));
    let h = handled.clone();
    let service = Arc::new(
        Service::new(())
            .event("order.created")
            .handle(move |ctx| {
                h.lock()
                    .unwrap()
                    .push(ctx.message().id().unwrap_or_default().to_string());
                Ok(json!({"ok": true}))
            })
            .event("flaky")
            .handle(|_| {
                Err(HandlerError::Repository(
                    sourced_rust::RepositoryError::Model("transient".into()),
                ))
            })
            .event("bad")
            .handle(|_| Err(HandlerError::Rejected("permanent".into()))),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = cloud_events_router(service);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}/"), handled)
}

#[tokio::test]
async fn binary_mode_success_returns_200_after_handler() {
    let (url, handled) = spawn_server().await;
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("ce-id", "evt-1")
        .header("ce-type", "order.created")
        .header("ce-source", "/orders")
        .header("content-type", "application/json")
        .body(r#"{"order":"o1"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(handled.lock().unwrap().clone(), vec!["evt-1".to_string()]);
}

#[tokio::test]
async fn structured_mode_success_returns_200() {
    let (url, handled) = spawn_server().await;
    let client = reqwest::Client::new();
    let event = json!({
        "specversion": "1.0",
        "id": "evt-2",
        "type": "order.created",
        "source": "/orders",
        "datacontenttype": "application/json",
        "data": {"order": "o2"},
    });
    let resp = client
        .post(&url)
        .header("content-type", "application/cloudevents+json")
        .body(event.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(handled.lock().unwrap().clone(), vec!["evt-2".to_string()]);
}

#[tokio::test]
async fn retryable_failure_returns_503() {
    let (url, _) = spawn_server().await;
    let resp = reqwest::Client::new()
        .post(&url)
        .header("ce-id", "evt-3")
        .header("ce-type", "flaky")
        .body("{}")
        .send()
        .await
        .unwrap();
    // Knative should redeliver.
    assert_eq!(resp.status(), 503);
}

#[tokio::test]
async fn permanent_failure_returns_422() {
    let (url, _) = spawn_server().await;
    let resp = reqwest::Client::new()
        .post(&url)
        .header("ce-id", "evt-4")
        .header("ce-type", "bad")
        .body("{}")
        .send()
        .await
        .unwrap();
    // Knative should not retry; its Delivery config dead-letters.
    assert_eq!(resp.status(), 422);
}

#[tokio::test]
async fn unknown_type_returns_422() {
    let (url, _) = spawn_server().await;
    let resp = reqwest::Client::new()
        .post(&url)
        .header("ce-id", "evt-5")
        .header("ce-type", "no.such.handler")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 422);
}

#[tokio::test]
async fn missing_id_returns_400() {
    let (url, _) = spawn_server().await;
    let resp = reqwest::Client::new()
        .post(&url)
        .header("ce-type", "order.created")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}
