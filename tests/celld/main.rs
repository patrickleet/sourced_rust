//! Live celld host: one Todo Durable Object per todo id, one Chat cell per
//! message id.
//!
//! Fixture checks always run. The HTTP round-trip runs only when `CELLD_URL`
//! is set (operator started compose + `celld deploy`). See `tests/celld/README.md`.

use std::path::Path;
use std::time::Duration;

use distributed::cell_host::{
    CELL_INTERNAL_SECRET_ENV, CELL_INTERNAL_SECRET_HEADER, CELL_PRINCIPAL_PARTITION_HEADER,
    CELL_SERVICE_ID_HEADER,
};
use serde_json::Value;

const TEST_SERVICE_ID: &str = "celld-live-test";
const TEST_PRINCIPAL_PARTITION: &str = "test-principal-alice";

#[path = "../support/env.rs"]
mod env_support;

fn worker_dir() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/celld/worker"))
}

fn relay_worker_dir() -> &'static Path {
    Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/celld/relay-worker"
    ))
}

#[test]
fn worker_declares_sqlite_todo_and_chat_cells() {
    let wrangler =
        std::fs::read_to_string(worker_dir().join("wrangler.jsonc")).expect("wrangler.jsonc");
    let spec: Value = serde_json::from_str(&wrangler).expect("wrangler json");
    assert_eq!(spec["main"], "build/worker/shim.mjs");
    let bindings = spec["durable_objects"]["bindings"].as_array().unwrap();
    assert_eq!(bindings[0]["name"], "TODO");
    assert_eq!(bindings[0]["class_name"], "TodoCell");
    assert_eq!(bindings[1]["name"], "CHAT");
    assert_eq!(bindings[1]["class_name"], "ChatCell");
    let v1 = spec["migrations"][0]["new_sqlite_classes"]
        .as_array()
        .unwrap();
    assert_eq!(v1[0], "TodoCell");
    let v2 = spec["migrations"][1]["new_sqlite_classes"]
        .as_array()
        .unwrap();
    assert_eq!(v2[0], "ChatCell");
    assert_eq!(spec["queues"]["producers"][0]["binding"], "EVENTS");
    assert_eq!(
        spec["queues"]["producers"][0]["queue"],
        "distributed-domain-events"
    );
    assert!(spec["vars"].get("OUTBOX_DRAIN_URL").is_none());

    let source = std::fs::read_to_string(worker_dir().join("src/lib.rs")).expect("lib.rs");
    assert!(source.contains("pub struct TodoCell"));
    assert!(source.contains("pub struct ChatCell"));
    assert!(source.contains("AggregateCell::<Todo>"));
    assert!(source.contains("AggregateCell::<ChatMessage>"));
    assert!(source.contains("id_from_name"));
    assert!(source.contains("mount(create())"));
    assert!(source.contains("mount(complete())"));
    assert!(source.contains("mount(post())"));
    assert!(source.contains("CREATE TABLE IF NOT EXISTS cell_events"));
    assert!(source.contains("CREATE TABLE IF NOT EXISTS cell_snapshots"));
    assert!(source.contains("CREATE TABLE IF NOT EXISTS cell_sealed"));
    assert!(source.contains("todo.create"));
    assert!(source.contains("todo.complete"));
    assert!(source.contains("chat.post"));
    assert!(source.contains("outbox.complete"));
    assert!(source.contains("outbox.claim"));
    assert!(source.contains("outbox.release"));
    assert!(source.contains("CREATE TABLE IF NOT EXISTS cell_outbox"));
    assert!(source.contains("CREATE TABLE IF NOT EXISTS cell_commands"));
    assert!(source.contains("CREATE TABLE IF NOT EXISTS cell_state"));
    assert!(source.contains("durable_state"));
    assert!(source.contains("restore_durable_state"));
    assert!(source.contains("ON CONFLICT(id) DO UPDATE SET body = excluded.body"));
    assert!(source.contains("dispatch_idempotent"));
    assert!(source.contains("CelldQueuePublisher::from_env"));
    assert!(source.contains("outbox_dispatcher"));
    assert!(source.contains("restore_durable_commands"));
    assert!(source.contains("sealed_row"));
    assert!(source.contains("new_with_snapshots"));
    assert!(source.contains("restore_durable_events"));
    assert!(source.contains("restore_durable_snapshots"));
    assert!(source.contains("restore_chat_copy"));
}

#[test]
fn relay_worker_consumes_events_queue_through_native_bus_boundary() {
    let wrangler = std::fs::read_to_string(relay_worker_dir().join("wrangler.jsonc"))
        .expect("relay wrangler.jsonc");
    let spec: Value = serde_json::from_str(&wrangler).expect("relay wrangler json");
    assert_eq!(
        spec["queues"]["consumers"][0]["queue"],
        "distributed-domain-events"
    );
    assert_eq!(
        spec["vars"]["CELLD_QUEUE_RELAY_URL"],
        "http://host.docker.internal:8791/internal/celld-queue/relay"
    );

    let source =
        std::fs::read_to_string(relay_worker_dir().join("src/lib.rs")).expect("relay lib.rs");
    assert!(source.contains("#[event(queue)]"));
    assert!(source.contains("CelldQueueRelay"));
    assert!(source.contains("CelldQueueHttpPublisher"));
    assert!(!source.contains("event(fetch)"));
}

#[test]
fn compose_uses_celld_0_4_local_store_with_persistent_volume() {
    let compose = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/celld/docker-compose.yml"
    ))
    .expect("compose");
    assert!(compose.contains("ghcr.io/denoland/celld:0.4.0"));
    assert!(compose.contains("- dev"));
    assert!(compose.contains("celld-dev-data:/worker/.celld"));
    assert!(!compose.contains("azurite"));
    assert!(!compose.contains("AZURE_STORAGE"));
    assert!(!compose.contains("s3"));
    assert!(compose.contains("CELLD_HTTP_PORT:-18080"));
    assert!(compose.contains("127.0.0.1:${CELLD_HTTP_PORT:-18080}:8080"));
    assert!(compose.contains(":8080"));
}

#[tokio::test]
async fn live_cell_private_routes_reject_missing_forged_and_malformed_authority() {
    let Some(base) = env_support::broker_env("CELLD_URL", "celld live security boundary") else {
        return;
    };
    let base = base.trim_end_matches('/').to_string();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("client");
    wait_healthy(&client, &base).await;
    let (id, _) = unique_cell_pair("todo");
    let command = serde_json::json!({
        "commandId": "0190a000-0000-7000-8000-000000000199",
        "input": { "title": "must not be created" }
    });

    let missing = client
        .post(format!("{base}/todo/{id}/todo.create"))
        .header(CELL_SERVICE_ID_HEADER, TEST_SERVICE_ID)
        .header(CELL_PRINCIPAL_PARTITION_HEADER, TEST_PRINCIPAL_PARTITION)
        .header("x-user-id", "alice")
        .header("x-roles", "admin")
        .json(&command)
        .send()
        .await
        .expect("missing secret");
    assert_eq!(missing.status(), 401);

    let forged = client
        .post(format!("{base}/todo/{id}/todo.create"))
        .header(
            CELL_INTERNAL_SECRET_HEADER,
            "forged-secret-for-red-team-request",
        )
        .header(CELL_SERVICE_ID_HEADER, TEST_SERVICE_ID)
        .header(CELL_PRINCIPAL_PARTITION_HEADER, TEST_PRINCIPAL_PARTITION)
        .header("x-user-id", "alice")
        .header("x-roles", "admin")
        .json(&command)
        .send()
        .await
        .expect("forged secret");
    assert_eq!(forged.status(), 401);

    let read = client
        .get(format!("{base}/todo/{id}"))
        .send()
        .await
        .expect("unauthenticated read");
    assert_eq!(read.status(), 401);

    let malformed = trusted_cell_request(client.post(format!("{base}/todo/{id}/outbox.claim")))
        .json(&serde_json::json!({
            "workerId": "attacker",
            "limit": 1,
            "leaseMs": 30_000,
            "forged": true
        }))
        .send()
        .await
        .expect("malformed claim");
    assert_eq!(malformed.status(), 400);
}

#[tokio::test]
async fn live_todo_cell_create_complete_reopen_archive_and_isolate() {
    let Some(base) = env_support::broker_env("CELLD_URL", "celld live Todo cell") else {
        return;
    };
    let base = base.trim_end_matches('/').to_string();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("client");

    wait_healthy(&client, &base).await;

    let (a, b) = unique_cell_pair("todo");

    let created = trusted_cell_request(
        client
            .post(format!("{base}/todo/{a}/todo.create"))
            .header("x-user-id", "alice")
            .header("x-roles", "user"),
    )
    .json(&serde_json::json!({
        "commandId": "0190a000-0000-7000-8000-000000000201",
        "input": { "title": "ship celld" }
    }))
    .send()
    .await
    .expect("create");
    assert_eq!(created.status(), 201, "{}", created.text().await.unwrap());
    let created: Value = created.json().await.unwrap();
    assert_eq!(created["payload"]["id"], a);
    assert_eq!(created["payload"]["status"], "open");
    assert_eq!(
        created["receipt"]["commandId"],
        "0190a000-0000-7000-8000-000000000201"
    );
    let causation_id = created["receipt"]["causationId"]
        .as_str()
        .expect("causationId")
        .to_string();

    let replay = trusted_cell_request(
        client
            .post(format!("{base}/todo/{a}/todo.create"))
            .header("x-user-id", "alice")
            .header("x-roles", "user"),
    )
    .json(&serde_json::json!({
        "commandId": "0190a000-0000-7000-8000-000000000201",
        "input": { "title": "ship celld" }
    }))
    .send()
    .await
    .expect("replay create");
    assert_eq!(replay.status(), 201, "{}", replay.text().await.unwrap());
    let replay: Value = replay.json().await.unwrap();
    assert_eq!(replay["receipt"]["replayed"], true);
    assert_eq!(replay["receipt"]["causationId"], causation_id);
    assert_eq!(replay["payload"], created["payload"]);

    let conflict = trusted_cell_request(
        client
            .post(format!("{base}/todo/{a}/todo.create"))
            .header("x-user-id", "alice")
            .header("x-roles", "user"),
    )
    .json(&serde_json::json!({
        "commandId": "0190a000-0000-7000-8000-000000000201",
        "input": { "title": "different input" }
    }))
    .send()
    .await
    .expect("conflicting create");
    assert_eq!(conflict.status(), 409, "{}", conflict.text().await.unwrap());
    let conflict: Value = conflict.json().await.unwrap();
    assert_eq!(conflict["code"], "COMMAND_ID_REUSE");

    let completed = trusted_cell_request(
        client
            .post(format!("{base}/todo/{a}/todo.complete"))
            .header("x-user-id", "alice")
            .header("x-roles", "user"),
    )
    .json(&serde_json::json!({
        "commandId": "0190a000-0000-7000-8000-000000000202",
        "input": {}
    }))
    .send()
    .await
    .expect("complete");
    assert_eq!(
        completed.status(),
        200,
        "{}",
        completed.text().await.unwrap()
    );
    let completed: Value = completed.json().await.unwrap();
    assert_eq!(completed["payload"]["status"], "completed");
    assert_eq!(
        completed["receipt"]["commandId"],
        "0190a000-0000-7000-8000-000000000202"
    );

    let reopened = trusted_cell_request(
        client
            .post(format!("{base}/todo/{a}/todo.reopen"))
            .header("x-user-id", "alice")
            .header("x-roles", "user"),
    )
    .json(&serde_json::json!({
        "commandId": "0190a000-0000-7000-8000-000000000203",
        "input": {}
    }))
    .send()
    .await
    .expect("reopen");
    assert_eq!(reopened.status(), 200, "{}", reopened.text().await.unwrap());
    let reopened: Value = reopened.json().await.unwrap();
    assert_eq!(reopened["payload"]["status"], "open");

    let archived = trusted_cell_request(
        client
            .post(format!("{base}/todo/{a}/todo.archive"))
            .header("x-user-id", "alice")
            .header("x-roles", "user"),
    )
    .json(&serde_json::json!({
        "commandId": "0190a000-0000-7000-8000-000000000204",
        "input": {}
    }))
    .send()
    .await
    .expect("archive");
    assert_eq!(archived.status(), 200, "{}", archived.text().await.unwrap());
    let archived: Value = archived.json().await.unwrap();
    assert_eq!(archived["payload"]["status"], "archived");

    let got: Value = trusted_cell_request(client.get(format!("{base}/todo/{a}")))
        .send()
        .await
        .expect("get")
        .json()
        .await
        .unwrap();
    assert_eq!(got["title"], "ship celld");
    assert_eq!(got["status"], "archived");

    let other = trusted_cell_request(client.get(format!("{base}/todo/{b}")))
        .send()
        .await
        .expect("missing cell");
    assert_eq!(other.status(), 404, "second name must be a different cell");
}

#[tokio::test]
async fn live_chat_cell_post_and_isolate() {
    let Some(base) = env_support::broker_env("CELLD_URL", "celld live Chat cell") else {
        return;
    };
    let base = base.trim_end_matches('/').to_string();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("client");

    wait_healthy(&client, &base).await;

    let (a, b) = unique_cell_pair("chat");
    let created_at = unix_millis();

    let posted = trusted_cell_request(
        client
            .post(format!("{base}/chat/{a}/chat.post"))
            .header("x-user-id", "alice")
            .header("x-roles", "user"),
    )
    .json(&serde_json::json!({
        "commandId": "0190a000-0000-7000-8000-000000000301",
        "input": {
            "message_id": a,
            "room_id": "lobby",
            "body": "hello from a cell",
            "created_at": created_at,
        }
    }))
    .send()
    .await
    .expect("post");
    assert_eq!(posted.status(), 201, "{}", posted.text().await.unwrap());
    let posted: Value = posted.json().await.unwrap();
    assert_eq!(posted["payload"]["message_id"], a);
    assert_eq!(posted["payload"]["body"], "hello from a cell");
    assert_eq!(posted["payload"]["author_id"], "alice");
    assert_eq!(
        posted["receipt"]["commandId"],
        "0190a000-0000-7000-8000-000000000301"
    );

    let got: Value = trusted_cell_request(client.get(format!("{base}/chat/{a}")))
        .send()
        .await
        .expect("get")
        .json()
        .await
        .unwrap();
    assert_eq!(got["body"], "hello from a cell");
    assert_eq!(got["author_id"], "alice");
    assert_eq!(got["room_id"], "lobby");

    let pending = posted["outbox"].as_array().cloned().unwrap_or_default();
    if !pending.is_empty() {
        let ids: Vec<Value> = pending
            .iter()
            .filter_map(|row| row.get("id").cloned())
            .collect();
        let worker_id = "celld-live-test-worker";
        let claim = trusted_cell_request(client.post(format!("{base}/chat/{a}/outbox.claim")))
            .json(&serde_json::json!({
                "workerId": worker_id,
                "limit": 64,
                "leaseMs": 30_000
            }))
            .send()
            .await
            .expect("outbox.claim");
        assert_eq!(claim.status(), 200, "{}", claim.text().await.unwrap());
        let claim: Value = claim.json().await.unwrap();
        assert_eq!(claim["outbox"].as_array().map(Vec::len), Some(ids.len()));
        assert!(claim["outbox"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["status"] == "in_flight"));

        let stale = trusted_cell_request(client.post(format!("{base}/chat/{a}/outbox.complete")))
            .json(&serde_json::json!({ "workerId": "wrong-worker", "ids": ids }))
            .send()
            .await
            .expect("stale completion");
        assert_eq!(stale.status(), 409);

        let complete =
            trusted_cell_request(client.post(format!("{base}/chat/{a}/outbox.complete")))
                .json(&serde_json::json!({ "workerId": worker_id, "ids": ids }))
                .send()
                .await
                .expect("outbox.complete");
        assert_eq!(complete.status(), 200, "{}", complete.text().await.unwrap());

        let drained: Value =
            trusted_cell_request(client.post(format!("{base}/chat/{a}/outbox.claim")))
                .json(&serde_json::json!({
                    "workerId": worker_id,
                    "limit": 64,
                    "leaseMs": 30_000
                }))
                .send()
                .await
                .expect("second outbox.claim")
                .json()
                .await
                .unwrap();
        assert_eq!(drained["outbox"].as_array().map(Vec::len).unwrap_or(0), 0);
    }

    let other = trusted_cell_request(client.get(format!("{base}/chat/{b}")))
        .send()
        .await
        .expect("missing cell");
    assert_eq!(other.status(), 404, "second name must be a different cell");
}

fn unique_cell_pair(kind: &str) -> (String, String) {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    (format!("{kind}-{nanos}-a"), format!("{kind}-{nanos}-b"))
}

fn unix_millis() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis()
        .to_string()
}

fn trusted_cell_request(request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    let secret = std::env::var(CELL_INTERNAL_SECRET_ENV)
        .unwrap_or_else(|_| "test-only-internal-secret-change-me-2026".into());
    request
        .header(CELL_INTERNAL_SECRET_HEADER, secret)
        .header(CELL_SERVICE_ID_HEADER, TEST_SERVICE_ID)
        .header(CELL_PRINCIPAL_PARTITION_HEADER, TEST_PRINCIPAL_PARTITION)
}

async fn wait_healthy(client: &reqwest::Client, base: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        for path in ["/health", "/__celld/health", "/"] {
            if let Ok(response) = client.get(format!("{base}{path}")).send().await {
                if response.status().is_success() {
                    return;
                }
            }
        }
        if std::time::Instant::now() > deadline {
            panic!("celld at {base} did not become healthy in 30s");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}
