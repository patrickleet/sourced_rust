//! Live celld host: one Todo Durable Object per id.
//!
//! Fixture checks always run. The HTTP round-trip runs only when `CELLD_URL`
//! is set (operator started compose + `celld deploy`). See `tests/celld/README.md`.

use std::path::Path;
use std::time::Duration;

use serde_json::Value;

#[path = "../support/env.rs"]
mod env_support;

fn worker_dir() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/celld/worker"))
}

#[test]
fn worker_declares_sqlite_todo_cell() {
    let wrangler =
        std::fs::read_to_string(worker_dir().join("wrangler.jsonc")).expect("wrangler.jsonc");
    let spec: Value = serde_json::from_str(&wrangler).expect("wrangler json");
    assert_eq!(spec["main"], "build/worker/shim.mjs");
    let bindings = spec["durable_objects"]["bindings"].as_array().unwrap();
    assert_eq!(bindings[0]["name"], "TODO");
    assert_eq!(bindings[0]["class_name"], "TodoCell");
    let classes = spec["migrations"][0]["new_sqlite_classes"]
        .as_array()
        .unwrap();
    assert_eq!(classes[0], "TodoCell");

    let source = std::fs::read_to_string(worker_dir().join("src/lib.rs")).expect("lib.rs");
    assert!(source.contains("pub struct TodoCell"));
    assert!(source.contains("AggregateCell::<Todo>"));
    assert!(source.contains("id_from_name"));
    assert!(source.contains("mount(create())"));
    assert!(source.contains("mount(complete())"));
    assert!(source.contains("CREATE TABLE IF NOT EXISTS cell_events"));
    assert!(source.contains("restore_durable_events"));
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
}

#[tokio::test]
async fn live_todo_cell_create_complete_and_isolate() {
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

    let created = client
        .put(format!("{base}/todo/{a}"))
        .json(&serde_json::json!({ "title": "ship celld" }))
        .send()
        .await
        .expect("create");
    assert_eq!(created.status(), 201, "{}", created.text().await.unwrap());
    let created: Value = created.json().await.unwrap();
    assert_eq!(created["id"], a);
    assert_eq!(created["status"], "open");

    let completed = client
        .post(format!("{base}/todo/{a}/complete"))
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
    assert_eq!(completed["status"], "completed");

    let got: Value = client
        .get(format!("{base}/todo/{a}"))
        .send()
        .await
        .expect("get")
        .json()
        .await
        .unwrap();
    assert_eq!(got["title"], "ship celld");
    assert_eq!(got["status"], "completed");

    let other = client
        .get(format!("{base}/todo/{b}"))
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
