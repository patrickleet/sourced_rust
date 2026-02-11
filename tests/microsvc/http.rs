//! HTTP transport integration tests.
//!
//! Starts an axum server and exercises it with reqwest.

use std::sync::Arc;

use serde_json::json;
use sourced_rust::microsvc::{self, Service};
use sourced_rust::HashMapRepository;

fn test_service() -> Arc<Service<HashMapRepository>> {
    Arc::new(
        Service::new(HashMapRepository::new())
            .command("ping", |_ctx| Ok(json!({ "pong": true })))
            .command("echo", |ctx| {
                let input = ctx.raw_input().clone();
                Ok(input)
            })
            .command("whoami", |ctx| {
                let user_id = ctx.user_id()?;
                Ok(json!({ "user_id": user_id }))
            }),
    )
}

/// Bind to port 0 and return the actual address.
async fn start_server(
    service: Arc<Service<HashMapRepository>>,
) -> String {
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
    let base = start_server(test_service()).await;
    let client = reqwest::Client::new();

    let resp = client.get(format!("{base}/health")).send().await.unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], true);
    assert!(body["commands"].is_array());
}

#[tokio::test]
async fn post_command() {
    let base = start_server(test_service()).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{base}/ping"))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body, json!({ "pong": true }));
}

#[tokio::test]
async fn post_with_body() {
    let base = start_server(test_service()).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{base}/echo"))
        .json(&json!({ "hello": "world" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body, json!({ "hello": "world" }));
}

#[tokio::test]
async fn unknown_command_returns_404() {
    let base = start_server(test_service()).await;
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
    let base = start_server(test_service()).await;
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
    let base = start_server(test_service()).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{base}/whoami"))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}
