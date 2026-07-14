//! Behavioral suite — GraphQL-only commands + queries, per-user isolation.
//!
//! Set `E2E_BASE_URL` to hit an external process, or leave unset to boot
//! an in-process service (SQLite memory + InMemoryBus + projector loop).

use std::sync::Arc;
use std::time::Duration;

use distributed::bus::{InMemoryBus, RunOptions};
use distributed::microsvc::serve;
use distributed::{SqliteLockManager, SqliteRepository};
use e2e_service::{
    build_graphql_engine, build_service, dev_identity, distributed_manifest,
};
use e2e_suite::{
    assert_http_commands_disabled, cases, graphql, graphql_raw, todos_archive, todos_complete,
    todos_create, todos_force_archive, todos_rename, todos_reopen, wait_ready,
};

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
async fn t0_http_command_routes_disabled() {
    let base = ensure_target().await;
    assert_http_commands_disabled(&base)
        .await
        .unwrap_or_else(|e| panic!("{}: {e}", cases::HTTP_OFF));
    eprintln!("{} ok", cases::HTTP_OFF);
}

#[tokio::test]
async fn t1_create_and_project() {
    let base = ensure_target().await;
    let tid = id("t");
    let resp = todos_create(&base, &tid, "Buy milk", "alice", "user")
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

/// Create via GraphQL command mutation; owner is always session user.
#[tokio::test]
async fn t1b_create_via_graphql_mutation() {
    let base = ensure_target().await;
    let tid = id("tgql");
    let payload = todos_create(&base, &tid, "Via GQL", "alice", "user")
        .await
        .unwrap_or_else(|e| panic!("todos_create mutation: {e}"));
    assert_eq!(payload["todo_id"], tid);
    assert_eq!(payload["owner_id"], "alice");
    assert_eq!(payload["title"], "Via GQL");

    let row = poll_todo(&base, "alice", &tid)
        .await
        .expect("mutation must project into todos read model");
    assert_eq!(row["owner_id"], "alice");
    eprintln!("todos_create ok {tid}");
}

/// GraphQL create input has no owner_id; session principal is always the owner.
#[tokio::test]
async fn t1c_create_owner_from_session_not_input() {
    let base = ensure_target().await;
    let tid = id("towner");
    // Extra owner_id field must not be accepted as spoof — schema rejects or ignores.
    let spoof = format!(
        r#"mutation {{
          todos_create(input: {{ todo_id: "{tid}", title: "owned", owner_id: "evil" }}) {{
            todo_id owner_id
          }}
        }}"#
    );
    let spoof_res = graphql(&base, &spoof, "alice", "user").await;
    match spoof_res {
        Ok(v) => {
            // If engine ignores unknown input fields, owner must still be session.
            if let Some(owner) = v["data"]["todos_create"]["owner_id"].as_str() {
                assert_eq!(
                    owner,
                    "alice",
                    "{}: owner spoofed: {v}",
                    cases::CREATE_OWNER_SESSION
                );
            } else {
                // GraphQL errors (unknown field) also acceptable
                let errs = v.get("errors").and_then(|e| e.as_array());
                assert!(
                    errs.map(|a| !a.is_empty()).unwrap_or(false),
                    "{}: expected errors or alice owner: {v}",
                    cases::CREATE_OWNER_SESSION
                );
            }
        }
        Err(e) => {
            assert!(
                e.to_lowercase().contains("error")
                    || e.contains("400")
                    || e.contains("field")
                    || e.contains("owner"),
                "{}: unexpected: {e}",
                cases::CREATE_OWNER_SESSION
            );
        }
    }

    let tid2 = id("towner2");
    let payload = todos_create(&base, &tid2, "session owner", "alice", "user")
        .await
        .expect(cases::CREATE_OWNER_SESSION);
    assert_eq!(payload["owner_id"], "alice");
    eprintln!("{} ok", cases::CREATE_OWNER_SESSION);
}

#[tokio::test]
async fn t2_owner_isolation_graphql() {
    let base = ensure_target().await;
    let alice_id = id("ta");
    let bob_id = id("tb");

    todos_create(&base, &alice_id, "Alice only", "alice", "user")
        .await
        .expect(cases::OWNER_ISOLATION);
    todos_create(&base, &bob_id, "Bob only", "bob", "user")
        .await
        .expect(cases::OWNER_ISOLATION);

    assert!(poll_todo(&base, "alice", &alice_id).await.is_some());
    assert!(poll_todo(&base, "bob", &bob_id).await.is_some());

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

/// Admin role: same `todos` query has no owner filter — sees alice + bob rows.
#[tokio::test]
async fn t2b_admin_sees_all_owners() {
    let base = ensure_target().await;
    let alice_id = id("taa");
    let bob_id = id("tbb");

    todos_create(&base, &alice_id, "Alice note", "alice", "user")
        .await
        .expect(cases::ADMIN_SEES_ALL);
    todos_create(&base, &bob_id, "Bob note", "bob", "user")
        .await
        .expect(cases::ADMIN_SEES_ALL);

    assert!(poll_todo(&base, "alice", &alice_id).await.is_some());
    assert!(poll_todo(&base, "bob", &bob_id).await.is_some());

    let v = graphql(
        &base,
        "{ todos { todo_id owner_id title } }",
        "admin-user",
        "admin",
    )
    .await
    .expect(cases::ADMIN_SEES_ALL);
    let arr = v["data"]["todos"].as_array().expect("todos array");
    let owners: Vec<&str> = arr
        .iter()
        .filter_map(|r| r["owner_id"].as_str())
        .collect();
    assert!(
        arr.iter().any(|r| r["todo_id"] == alice_id),
        "{}: admin missing alice todo: {v}",
        cases::ADMIN_SEES_ALL
    );
    assert!(
        arr.iter().any(|r| r["todo_id"] == bob_id),
        "{}: admin missing bob todo: {v}",
        cases::ADMIN_SEES_ALL
    );
    assert!(
        owners.contains(&"alice") && owners.contains(&"bob"),
        "{}: admin should see multiple owners, got {owners:?}",
        cases::ADMIN_SEES_ALL
    );
    eprintln!("{} ok alice={alice_id} bob={bob_id}", cases::ADMIN_SEES_ALL);
}

/// Admin-only mutation: force-archive another user's todo; user role cannot call it.
#[tokio::test]
async fn t2c_admin_force_archive() {
    let base = ensure_target().await;
    let tid = id("tfa");

    todos_create(&base, &tid, "Alice will be forced", "alice", "user")
        .await
        .expect(cases::ADMIN_FORCE_ARCHIVE);
    assert!(poll_todo(&base, "alice", &tid).await.is_some());

    // User role must not force-archive (field absent / rejected).
    let user_attempt = todos_force_archive(&base, &tid, "alice", "user").await;
    assert!(
        user_attempt.is_err(),
        "{}: user role must not force-archive (got ok: {user_attempt:?})",
        cases::ADMIN_FORCE_ARCHIVE
    );
    // Denied call must not archive the row.
    let after_deny = poll_todo(&base, "alice", &tid)
        .await
        .expect("todo exists after denied force-archive");
    assert_ne!(
        after_deny["status"], "archived",
        "{}: user force-archive must not change status",
        cases::ADMIN_FORCE_ARCHIVE
    );

    let payload = todos_force_archive(&base, &tid, "admin-user", "admin")
        .await
        .unwrap_or_else(|e| panic!("{}: {e}", cases::ADMIN_FORCE_ARCHIVE));
    assert_eq!(payload["todo_id"], tid);
    assert_eq!(payload["owner_id"], "alice");
    assert_eq!(payload["status"], "archived");
    assert_eq!(payload["archived_by"], "admin-user");

    let mut ok = false;
    for _ in 0..100 {
        if let Some(row) = poll_todo(&base, "alice", &tid).await {
            if row["status"] == "archived" {
                ok = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    assert!(
        ok,
        "{}: projected status not archived after admin force",
        cases::ADMIN_FORCE_ARCHIVE
    );
    eprintln!("{} ok {tid}", cases::ADMIN_FORCE_ARCHIVE);
}

/// Role SDL: force-archive only on admin schema (engine source of truth).
#[tokio::test]
async fn t2d_sdl_role_split_force_archive() {
    let repo = distributed::SqliteRepository::connect_and_migrate("sqlite::memory:")
        .await
        .expect("repo");
    let registry = distributed_manifest().table_registry().expect("registry");
    repo.bootstrap_table_schema_for_dev(&registry)
        .await
        .expect("bootstrap");
    let engine = build_graphql_engine(repo.pool().clone(), dev_identity(), None).expect("gql");
    let user_sdl = engine.sdl_for_role("user").expect("user sdl");
    let admin_sdl = engine.sdl_for_role("admin").expect("admin sdl");
    assert!(
        !user_sdl.contains("todos_force_archive"),
        "{}: user SDL must not expose force-archive",
        cases::SDL_ROLE_SPLIT
    );
    assert!(
        admin_sdl.contains("todos_force_archive"),
        "{}: admin SDL must expose force-archive",
        cases::SDL_ROLE_SPLIT
    );
    assert!(
        admin_sdl.contains("todo.force_archived")
            || admin_sdl.contains("TodoForceArchive")
            || admin_sdl.contains("todos_force_archive"),
        "{}: admin SDL mutation surface incomplete",
        cases::SDL_ROLE_SPLIT
    );
    eprintln!("{} ok", cases::SDL_ROLE_SPLIT);
}

/// Admin todos query supports limit (bounded list).
#[tokio::test]
async fn t2e_admin_todos_respects_limit() {
    let base = ensure_target().await;
    for i in 0..5 {
        let tid = id(&format!("lim{i}"));
        todos_create(&base, &tid, &format!("n{i}"), "alice", "user")
            .await
            .expect(cases::ADMIN_QUERY_LIMIT);
        assert!(poll_todo(&base, "alice", &tid).await.is_some());
    }
    let v = graphql(
        &base,
        "{ todos(limit: 2, order_by: [{ todo_id: asc }]) { todo_id } }",
        "admin-user",
        "admin",
    )
    .await
    .expect(cases::ADMIN_QUERY_LIMIT);
    let arr = v["data"]["todos"].as_array().expect("todos");
    assert!(
        arr.len() <= 2,
        "{}: expected at most 2 rows, got {}: {v}",
        cases::ADMIN_QUERY_LIMIT,
        arr.len()
    );
    eprintln!("{} ok n={}", cases::ADMIN_QUERY_LIMIT, arr.len());
}

#[tokio::test]
async fn t3_complete_projects_status() {
    let base = ensure_target().await;
    let tid = id("tc");
    todos_create(&base, &tid, "Do thing", "alice", "user")
        .await
        .expect(cases::COMPLETE);
    assert!(poll_todo(&base, "alice", &tid).await.is_some());

    todos_complete(&base, &tid, "alice", "user")
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
    todos_create(&base, &tid, "Alice task", "alice", "user")
        .await
        .expect(cases::NOT_OWNER);

    let err = todos_complete(&base, &tid, "bob", "user")
        .await
        .expect_err(cases::NOT_OWNER);
    assert!(
        err.to_lowercase().contains("reject")
            || err.to_lowercase().contains("owner")
            || err.to_lowercase().contains("forbidden")
            || err.contains("422")
            || err.contains("UNPROCESSABLE"),
        "{}: unexpected error: {err}",
        cases::NOT_OWNER
    );
    // Rename/archive/reopen IDOR: bob must not succeed (same domain owner gate as complete).
    assert!(
        todos_rename(&base, &tid, "Hijacked", "bob", "user")
            .await
            .is_err(),
        "{}: rename should fail for bob",
        cases::NOT_OWNER_MUTATES
    );
    assert!(
        todos_archive(&base, &tid, "bob", "user").await.is_err(),
        "{}: archive should fail for bob",
        cases::NOT_OWNER_MUTATES
    );
    // GraphQL todos_reopen path — real mutation helper, non-owner rejected.
    assert!(
        todos_reopen(&base, &tid, "bob", "user").await.is_err(),
        "{}: reopen should fail for bob",
        cases::NOT_OWNER_MUTATES
    );
    let row = poll_todo(&base, "alice", &tid).await.expect("row");
    assert_eq!(row["title"], "Alice task");
    assert_eq!(row["status"], "open");
    eprintln!("{} + {} ok", cases::NOT_OWNER, cases::NOT_OWNER_MUTATES);
}

#[tokio::test]
async fn t5_unauthenticated_rejected() {
    let base = ensure_target().await;
    let tid = id("tu");
    let doc = format!(
        r#"mutation {{
          todos_create(input: {{ todo_id: "{tid}", title: "no user" }}) {{
            todo_id
          }}
        }}"#
    );
    // DevHeaders with no identity → mutation fails (require_user).
    let (status, body) = graphql_raw(&base, &doc).await.expect(cases::UNAUTH);
    // Prefer GraphQL errors over HTTP 401 depending on identity mode.
    let has_err = body
        .get("errors")
        .and_then(|e| e.as_array())
        .map(|a| !a.is_empty())
        .unwrap_or(false);
    assert!(
        status == 401 || has_err,
        "{}: expected 401 or GraphQL errors, got HTTP {status} {body}",
        cases::UNAUTH
    );
    eprintln!("{} ok", cases::UNAUTH);
}

#[tokio::test]
async fn t6_lifecycle_rename_and_archive() {
    let base = ensure_target().await;
    let tid = id("tl");
    todos_create(&base, &tid, "Draft", "alice", "user")
        .await
        .expect(cases::LIFECYCLE);
    assert!(poll_todo(&base, "alice", &tid).await.is_some());

    todos_rename(&base, &tid, "Renamed", "alice", "user")
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

    todos_archive(&base, &tid, "alice", "user")
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

    let err = todos_complete(&base, &tid, "alice", "user")
        .await
        .expect_err("complete after archive");
    assert!(
        err.to_lowercase().contains("reject")
            || err.to_lowercase().contains("archiv")
            || err.contains("UNPROCESSABLE"),
        "{}: complete after archive: {err}",
        cases::LIFECYCLE
    );
    eprintln!("{} ok {tid}", cases::LIFECYCLE);
}
