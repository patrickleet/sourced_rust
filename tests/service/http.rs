//! HTTP integration tests for DomainService.
//!
//! Test A: dispatch() without a server — verify CommandRequest → CommandResponse mapping.
//! Test B: Full axum HTTP roundtrip with reqwest client.

use std::sync::Arc;

use sourced_rust::{
    CommandRequest, CommitAggregate, DomainService, GetAggregate, HandlerError,
    HashMapRepository,
};

use crate::support::{Counter, CreateCounter, DecrementCounter, IncrementCounter};

/// Build a DomainService with Counter handlers that use json_decode.
fn build_service() -> DomainService<HashMapRepository> {
    let repo = HashMapRepository::new();
    DomainService::new(repo)
        .command("counter.create", |repo, msg| {
            let data: CreateCounter = msg.json_decode()?;
            let mut counter = Counter::default();
            counter.create(data.id);
            repo.commit_aggregate(&mut counter)?;
            Ok(())
        })
        .command("counter.increment", |repo, msg| {
            let data: IncrementCounter = msg.json_decode()?;
            let mut counter: Counter = repo
                .get_aggregate(&data.id)?
                .ok_or_else(|| HandlerError::NotFound(data.id.clone()))?;
            counter.increment(data.amount);
            repo.commit_aggregate(&mut counter)?;
            Ok(())
        })
        .command("counter.decrement", |repo, msg| {
            let data: DecrementCounter = msg.json_decode()?;
            let mut counter: Counter = repo
                .get_aggregate(&data.id)?
                .ok_or_else(|| HandlerError::NotFound(data.id.clone()))?;
            counter.decrement(data.amount);
            repo.commit_aggregate(&mut counter)?;
            Ok(())
        })
}

// ============================================================================
// Test A: dispatch() without server
// ============================================================================

#[test]
fn dispatch_create_returns_200() {
    let service = build_service();

    let req = CommandRequest {
        id: Some("cmd-1".into()),
        command: "counter.create".into(),
        payload: serde_json::json!({ "id": "c1" }),
    };
    let resp = service.dispatch(&req);
    assert_eq!(resp.status, 200);

    // Verify aggregate was actually created
    let counter: Counter = service.repo().get_aggregate("c1").unwrap().unwrap();
    assert_eq!(counter.value, 0);
}

#[test]
fn dispatch_unknown_command_returns_404() {
    let service = build_service();

    let req = CommandRequest {
        id: Some("cmd-1".into()),
        command: "counter.nope".into(),
        payload: serde_json::json!({}),
    };
    let resp = service.dispatch(&req);
    assert_eq!(resp.status, 404);
}

#[test]
fn dispatch_not_found_returns_404() {
    let service = build_service();

    let req = CommandRequest {
        id: Some("cmd-1".into()),
        command: "counter.increment".into(),
        payload: serde_json::json!({ "id": "nonexistent", "amount": 1 }),
    };
    let resp = service.dispatch(&req);
    assert_eq!(resp.status, 404);
}

#[test]
fn dispatch_bad_payload_returns_400() {
    let service = build_service();

    // "counter.create" expects { id: String } — send a number instead
    let req = CommandRequest {
        id: Some("cmd-1".into()),
        command: "counter.create".into(),
        payload: serde_json::json!(42),
    };
    let resp = service.dispatch(&req);
    assert_eq!(resp.status, 400);
}

#[test]
fn dispatch_auto_generates_id_when_omitted() {
    let service = build_service();

    let req = CommandRequest {
        id: None,
        command: "counter.create".into(),
        payload: serde_json::json!({ "id": "auto-id-test" }),
    };
    let resp = service.dispatch(&req);
    assert_eq!(resp.status, 200);

    let counter: Counter = service.repo().get_aggregate("auto-id-test").unwrap().unwrap();
    assert_eq!(counter.value, 0);
}

#[test]
fn dispatch_full_lifecycle() {
    let service = build_service();

    // Create
    let resp = service.dispatch(&CommandRequest {
        id: Some("cmd-1".into()),
        command: "counter.create".into(),
        payload: serde_json::json!({ "id": "c1" }),
    });
    assert_eq!(resp.status, 200);

    // Increment
    let resp = service.dispatch(&CommandRequest {
        id: Some("cmd-2".into()),
        command: "counter.increment".into(),
        payload: serde_json::json!({ "id": "c1", "amount": 5 }),
    });
    assert_eq!(resp.status, 200);

    // Decrement
    let resp = service.dispatch(&CommandRequest {
        id: Some("cmd-3".into()),
        command: "counter.decrement".into(),
        payload: serde_json::json!({ "id": "c1", "amount": 2 }),
    });
    assert_eq!(resp.status, 200);

    // Verify: 0 + 5 - 2 = 3
    let counter: Counter = service.repo().get_aggregate("c1").unwrap().unwrap();
    assert_eq!(counter.value, 3);
}

#[test]
fn dispatch_rejected_returns_422() {
    let repo = HashMapRepository::new();
    let service = DomainService::new(repo).command("always.reject", |_repo, _msg| {
        Err(HandlerError::Rejected("nope".into()))
    });

    let req = CommandRequest {
        id: Some("cmd-1".into()),
        command: "always.reject".into(),
        payload: serde_json::json!({}),
    };
    let resp = service.dispatch(&req);
    assert_eq!(resp.status, 422);
}

// ============================================================================
// Test B: Full axum HTTP roundtrip
// ============================================================================

#[tokio::test]
async fn axum_http_roundtrip() {
    use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
    use tokio::net::TcpListener;

    // --- Build service behind Arc ---
    let service = Arc::new(build_service());

    // --- Axum handler (inline — shows how any framework integrates) ---
    async fn handle_command(
        State(service): State<Arc<DomainService<HashMapRepository>>>,
        Json(req): Json<CommandRequest>,
    ) -> (StatusCode, Json<serde_json::Value>) {
        let resp = service.dispatch(&req);
        let status = StatusCode::from_u16(resp.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(resp.body))
    }

    let app = Router::new()
        .route("/commands", post(handle_command))
        .with_state(service.clone());

    // --- Bind to random port ---
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    // --- Spawn server ---
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = reqwest::Client::new();
    let url = format!("http://{}/commands", addr);

    // 1. Create counter
    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "command": "counter.create",
            "payload": { "id": "http-c1" }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // 2. Increment
    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "command": "counter.increment",
            "payload": { "id": "http-c1", "amount": 10 }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // 3. Decrement
    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "command": "counter.decrement",
            "payload": { "id": "http-c1", "amount": 3 }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // 4. Unknown command → 404
    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "command": "counter.nope",
            "payload": {}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);

    // 5. Not found → 404
    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "command": "counter.increment",
            "payload": { "id": "ghost", "amount": 1 }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);

    // 6. Verify aggregate state via repo: 0 + 10 - 3 = 7
    let counter: Counter = service.repo().get_aggregate("http-c1").unwrap().unwrap();
    assert_eq!(counter.value, 7);
}
