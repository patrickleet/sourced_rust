//! Behavioral suite — HTTP commands + GraphQL, per-user isolation.
//!
//! Set `E2E_BASE_URL` to hit an external process, or leave unset to boot
//! an in-process service (SQLite memory + InMemoryBus + projector loop).

use std::sync::Arc;
use std::time::Duration;

use distributed::bus::{InMemoryBus, RunOptions};
use distributed::microsvc::serve;
use distributed::{SqliteLockManager, SqliteRepository};
use serde_json::json;
use e2e_service::{
    build_graphql_engine, build_service, dev_identity, distributed_manifest,
};
use e2e_suite::{cases, graphql, post_command, post_command_raw, wait_ready};

async fn ensure_target() -> String {
    if let Ok(url) = std::env::var("E2E_BASE_URL") {
        if !url.is_empty() {
            assert!(
                wait_ready(&url, Duration::from_secs(30)).await,
                "E2E_BASE_URL={url} not ready"
            );
            return url;
        }
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let bind = addr.to_string();
    let base = format!("http://{bind}");

    // Always in-memory SQLite for offline suite (ignore compose DATABASE_URL=postgres).
    let repo = SqliteRepository::connect_and_migrate("sqlite::memory:")
        .await
        .expect("repo");
    let registry = distributed_manifest().table_registry().expect("registry");
    repo.bootstrap_table_schema_for_dev(&registry)
        .await
        .expect("bootstrap");
    let locks = SqliteLockManager::new(repo.pool().clone());
    let bus = InMemoryBus::new();

    let change_rx = repo.read_model_changes();
    // Offline suite always uses DevHeaders (ignore ambient OIDC_* from make up).
    let gql = build_graphql_engine(repo.pool().clone(), dev_identity(), Some(change_rx))
        .expect("gql");
    let service = Arc::new(
        build_service(repo.clone(), locks.clone(), repo.clone())
            .with_bus(bus.clone())
            .with_graphql(gql),
    );

    // Projector consumer (eventual consistency path — no command-side dual-write).
    let consumer_repo = repo.clone();
    let consumer_locks = locks.clone();
    let bus_c = bus.clone();
    tokio::spawn(async move {
        loop {
            let service = build_service(
                consumer_repo.clone(),
                consumer_locks.clone(),
                consumer_repo.clone(),
            )
            .with_bus(bus_c.clone());
            let _ = service.run(RunOptions::idempotent()).await;
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    });

    let svc = Arc::clone(&service);
    let bind_c = bind.clone();
    tokio::spawn(async move {
        let _ = serve(svc, &bind_c).await;
    });

    assert!(
        wait_ready(&base, Duration::from_secs(10)).await,
        "in-process todo service not ready at {base}"
    );
    base
}

async fn poll_todo(base: &str, user: &str, todo_id: &str) -> Option<serde_json::Value> {
    for _ in 0..100 {
        if let Ok(v) = graphql(
            base,
            "{ todos { todo_id owner_id title status } }",
            user,
            "user",
        )
        .await
        {
            if let Some(arr) = v["data"]["todos"].as_array() {
                if let Some(row) = arr.iter().find(|r| r["todo_id"] == todo_id) {
                    return Some(row.clone());
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    None
}

fn id(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let s = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{n:x}-{s:x}")
}

#[tokio::test]
async fn t1_create_and_project() {
    let base = ensure_target().await;
    let tid = id("t");
    let resp = post_command(
        &base,
        "todo.create",
        json!({ "todo_id": tid, "title": "Buy milk" }),
        "alice",
        "user",
    )
    .await
    .unwrap_or_else(|e| panic!("{}: {e}", cases::CREATE));
    assert_eq!(resp["todo_id"], tid);
    assert_eq!(resp["owner_id"], "alice");
    assert_eq!(resp["status"], "open");

    let row = poll_todo(&base, "alice", &tid)
        .await
        .unwrap_or_else(|| panic!("{}: not projected", cases::CREATE));
    assert_eq!(row["title"], "Buy milk");
    assert_eq!(row["owner_id"], "alice");
    eprintln!("{} ok {tid}", cases::CREATE);
}

#[tokio::test]
async fn t2_owner_isolation_graphql() {
    let base = ensure_target().await;
    let alice_id = id("ta");
    let bob_id = id("tb");

    post_command(
        &base,
        "todo.create",
        json!({ "todo_id": alice_id, "title": "Alice only" }),
        "alice",
        "user",
    )
    .await
    .expect(cases::OWNER_ISOLATION);
    post_command(
        &base,
        "todo.create",
        json!({ "todo_id": bob_id, "title": "Bob only" }),
        "bob",
        "user",
    )
    .await
    .expect(cases::OWNER_ISOLATION);

    assert!(poll_todo(&base, "alice", &alice_id).await.is_some());
    assert!(poll_todo(&base, "bob", &bob_id).await.is_some());

    // Alice GraphQL must not include Bob's todo.
    let v = graphql(
        &base,
        "{ todos { todo_id owner_id } }",
        "alice",
        "user",
    )
    .await
    .expect(cases::OWNER_ISOLATION);
    let arr = v["data"]["todos"].as_array().expect("todos array");
    assert!(
        arr.iter().all(|r| r["owner_id"] == "alice"),
        "{}: alice saw foreign todos: {v}",
        cases::OWNER_ISOLATION
    );
    assert!(
        arr.iter().any(|r| r["todo_id"] == alice_id),
        "{}: missing alice todo",
        cases::OWNER_ISOLATION
    );
    assert!(
        !arr.iter().any(|r| r["todo_id"] == bob_id),
        "{}: alice saw bob's todo",
        cases::OWNER_ISOLATION
    );
    eprintln!("{} ok", cases::OWNER_ISOLATION);
}

#[tokio::test]
async fn t3_complete_projects_status() {
    let base = ensure_target().await;
    let tid = id("tc");
    post_command(
        &base,
        "todo.create",
        json!({ "todo_id": tid, "title": "Do thing" }),
        "alice",
        "user",
    )
    .await
    .expect(cases::COMPLETE);
    assert!(poll_todo(&base, "alice", &tid).await.is_some());

    post_command(
        &base,
        "todo.complete",
        json!({ "todo_id": tid }),
        "alice",
        "user",
    )
    .await
    .expect(cases::COMPLETE);

    let mut ok = false;
    for _ in 0..100 {
        if let Some(row) = poll_todo(&base, "alice", &tid).await {
            if row["status"] == "completed" {
                ok = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    assert!(ok, "{}: status not completed in GraphQL", cases::COMPLETE);
    eprintln!("{} ok {tid}", cases::COMPLETE);
}

#[tokio::test]
async fn t4_not_owner_rejected() {
    let base = ensure_target().await;
    let tid = id("to");
    post_command(
        &base,
        "todo.create",
        json!({ "todo_id": tid, "title": "Alice task" }),
        "alice",
        "user",
    )
    .await
    .expect(cases::NOT_OWNER);

    let (status, body) = post_command_raw(
        &base,
        "todo.complete",
        json!({ "todo_id": tid }),
        Some("bob"),
        Some("user"),
    )
    .await
    .expect(cases::NOT_OWNER);
    assert_eq!(
        status, 422,
        "{}: expected 422 not-owner, got {status} {body}",
        cases::NOT_OWNER
    );
    eprintln!("{} ok", cases::NOT_OWNER);
}

#[tokio::test]
async fn t5_unauthenticated_rejected() {
    let base = ensure_target().await;
    let (status, body) = post_command_raw(
        &base,
        "todo.create",
        json!({ "todo_id": id("tu"), "title": "no user" }),
        None,
        None,
    )
    .await
    .expect(cases::UNAUTH);
    assert_eq!(
        status, 401,
        "{}: expected 401, got {status} {body}",
        cases::UNAUTH
    );
    eprintln!("{} ok", cases::UNAUTH);
}

#[tokio::test]
async fn t6_lifecycle_rename_and_archive() {
    let base = ensure_target().await;
    let tid = id("tl");
    post_command(
        &base,
        "todo.create",
        json!({ "todo_id": tid, "title": "Draft" }),
        "alice",
        "user",
    )
    .await
    .expect(cases::LIFECYCLE);
    assert!(poll_todo(&base, "alice", &tid).await.is_some());

    post_command(
        &base,
        "todo.rename",
        json!({ "todo_id": tid, "title": "Renamed" }),
        "alice",
        "user",
    )
    .await
    .expect(cases::LIFECYCLE);

    let mut renamed = false;
    for _ in 0..100 {
        if let Some(row) = poll_todo(&base, "alice", &tid).await {
            if row["title"] == "Renamed" {
                renamed = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    assert!(renamed, "{}: rename not projected", cases::LIFECYCLE);

    post_command(
        &base,
        "todo.archive",
        json!({ "todo_id": tid }),
        "alice",
        "user",
    )
    .await
    .expect(cases::LIFECYCLE);

    let mut archived = false;
    for _ in 0..100 {
        if let Some(row) = poll_todo(&base, "alice", &tid).await {
            if row["status"] == "archived" {
                archived = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    assert!(archived, "{}: archive not projected", cases::LIFECYCLE);

    // Complete after archive must fail.
    let (status, _) = post_command_raw(
        &base,
        "todo.complete",
        json!({ "todo_id": tid }),
        Some("alice"),
        Some("user"),
    )
    .await
    .unwrap();
    assert_eq!(status, 422, "{}: complete after archive", cases::LIFECYCLE);
    eprintln!("{} ok {tid}", cases::LIFECYCLE);
}
