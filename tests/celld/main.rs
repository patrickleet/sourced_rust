//! Live celld host: one Todo Durable Object per todo id, one Chat cell per
//! message id.
//!
//! Fixture checks always run. The HTTP round-trip runs only when `CELLD_URL`
//! is set (operator started compose + `celld deploy`). See `tests/celld/README.md`.

use std::path::Path;
use std::time::Duration;

use distributed::cell_host::{CELL_PRINCIPAL_PARTITION_HEADER, CELL_SERVICE_ID_HEADER};
use serde_json::Value;

const TEST_SERVICE_ID: &str = "celld-live-test";
const TEST_PRINCIPAL_PARTITION: &str = "test-principal-alice";

#[path = "../support/env.rs"]
mod env_support;

fn worker_dir() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/celld/worker"))
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
    assert_eq!(
        spec["vars"]["OUTBOX_DRAIN_URL"],
        "http://host.docker.internal:8791/internal/outbox/drain"
    );

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
    assert!(source.contains("outbox.drain"));
    assert!(source.contains("CREATE TABLE IF NOT EXISTS cell_outbox"));
    assert!(source.contains("CREATE TABLE IF NOT EXISTS cell_commands"));
    assert!(source.contains("dispatch_idempotent"));
    assert!(source.contains("restore_durable_commands"));
    assert!(source.contains("sealed_row"));
    assert!(source.contains("new_with_snapshots"));
    assert!(source.contains("restore_durable_events"));
    assert!(source.contains("restore_durable_snapshots"));
    assert!(source.contains("restore_chat_copy"));
}

#[test]
fn compose_file_does_not_use_minio() {
    let compose = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/celld/docker-compose.yml"
    ))
    .expect("compose");
    let dockerfile = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/celld/Dockerfile"
    ))
    .expect("dockerfile");
    assert!(dockerfile.contains("ghcr.io/denoland/celld"));
    assert!(dockerfile.contains("socat"));
    assert!(compose.contains("mcr.microsoft.com/azure-storage/azurite"));
    assert!(compose.contains("az://celld"));
    assert!(compose.contains("AZURE_STORAGE_USE_EMULATOR"));
    assert!(
        !compose
            .lines()
            .any(|line| line.trim_start().starts_with("network_mode")),
        "Docker Desktop extra_hosts cannot combine with network_mode"
    );
    assert!(
        !compose
            .lines()
            .any(|line| line.trim_start().starts_with("image:") && line.contains("minio")),
        "do not run MinIO as the celld bucket"
    );
    assert!(compose.contains("CELLD_HTTP_PORT:-18080"));
    assert!(compose.contains(":8080"));
    assert!(compose.contains("host.docker.internal:host-gateway"));
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

    let a = unique_todo();
    let b = unique_todo();

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

    let got: Value = client
        .get(format!("{base}/todo/{a}"))
        .send()
        .await
        .expect("get")
        .json()
        .await
        .unwrap();
    assert_eq!(got["title"], "ship celld");
    assert_eq!(got["status"], "archived");

    let other = client
        .get(format!("{base}/todo/{b}"))
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

    let a = unique_chat();
    let b = unique_chat();
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

    let got: Value = client
        .get(format!("{base}/chat/{a}"))
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
        let complete = client
            .post(format!("{base}/chat/{a}/outbox.complete"))
            .json(&serde_json::json!({ "ids": ids }))
            .send()
            .await
            .expect("outbox.complete");
        assert_eq!(complete.status(), 200, "{}", complete.text().await.unwrap());
        let drained: Value = client
            .post(format!("{base}/chat/{a}/outbox.drain"))
            .json(&serde_json::json!({
                "commandId": "drain",
                "input": {}
            }))
            .send()
            .await
            .expect("outbox.drain")
            .json()
            .await
            .unwrap();
        assert_eq!(drained["outbox"].as_array().map(Vec::len).unwrap_or(0), 0);
    }

    let other = client
        .get(format!("{base}/chat/{b}"))
        .send()
        .await
        .expect("missing cell");
    assert_eq!(other.status(), 404, "second name must be a different cell");
}

fn unique_todo() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    format!("todo-{nanos}")
}

fn unique_chat() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    format!("chat-{nanos}")
}

fn unix_millis() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis()
        .to_string()
}

fn trusted_cell_request(request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    request
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
