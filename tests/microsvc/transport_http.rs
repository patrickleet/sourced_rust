//! HTTP transport integration tests.
//!
//! Starts an axum server and exercises it with reqwest.

use std::sync::Arc;

use serde_json::json;
use sourced_rust::microsvc::{self, HandlerError, Service};
use sourced_rust::{AggregateBuilder, HashMapRepository};

use crate::support::{Counter, CreateCounter, IncrementCounter};

fn counter_service() -> Arc<Service<HashMapRepository>> {
    Arc::new(
        Service::new(HashMapRepository::new())
            .command("counter.create", |ctx| {
                let input = ctx.input::<CreateCounter>()?;
                let counter_repo = ctx.repo().clone().aggregate::<Counter>();
                let mut counter = Counter::default();
                counter.create(input.id.clone());
                counter_repo.commit(&mut counter)?;
                Ok(json!({ "id": input.id }))
            })
            .command("counter.increment", |ctx| {
                let input = ctx.input::<IncrementCounter>()?;
                let counter_repo = ctx.repo().clone().aggregate::<Counter>();
                let mut counter: Counter = counter_repo
                    .get(&input.id)?
                    .ok_or_else(|| HandlerError::NotFound(input.id.clone()))?;
                counter.increment(input.amount);
                counter_repo.commit(&mut counter)?;
                Ok(json!({ "id": input.id, "value": counter.value }))
            })
            .command("whoami", |ctx| {
                let user_id = ctx.user_id()?;
                Ok(json!({ "user_id": user_id }))
            }),
    )
}

/// Bind to port 0 and return the actual address.
async fn start_server(service: Arc<Service<HashMapRepository>>) -> String {
    let app = microsvc::router(service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn health_check() {
    let service = counter_service();
    let base = start_server(service).await;
    let client = reqwest::Client::new();

    let resp = client.get(format!("{base}/health")).send().await.unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], true);

    let commands = body["commands"].as_array().unwrap();
    assert!(commands.iter().any(|c| c == "counter.create"));
    assert!(commands.iter().any(|c| c == "counter.increment"));
}

#[tokio::test]
async fn create_counter() {
    let service = counter_service();
    let base = start_server(service).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{base}/counter.create"))
        .json(&json!({ "id": "c1" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body, json!({ "id": "c1" }));
}

#[tokio::test]
async fn create_and_increment_counter() {
    let service = counter_service();
    let base = start_server(service).await;
    let client = reqwest::Client::new();

    // Create
    let resp = client
        .post(format!("{base}/counter.create"))
        .json(&json!({ "id": "c1" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Increment
    let resp = client
        .post(format!("{base}/counter.increment"))
        .json(&json!({ "id": "c1", "amount": 5 }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body, json!({ "id": "c1", "value": 5 }));

    // Increment again
    let resp = client
        .post(format!("{base}/counter.increment"))
        .json(&json!({ "id": "c1", "amount": 3 }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body, json!({ "id": "c1", "value": 8 }));
}

#[tokio::test]
async fn increment_nonexistent_returns_404() {
    let service = counter_service();
    let base = start_server(service).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{base}/counter.increment"))
        .json(&json!({ "id": "nope", "amount": 1 }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn unknown_command_returns_404() {
    let service = counter_service();
    let base = start_server(service).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{base}/nonexistent"))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn headers_flow_to_session() {
    let service = counter_service();
    let base = start_server(service).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{base}/whoami"))
        .header("x-hasura-user-id", "user-42")
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body, json!({ "user_id": "user-42" }));
}

#[tokio::test]
async fn missing_session_returns_401() {
    let service = counter_service();
    let base = start_server(service).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{base}/whoami"))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}
