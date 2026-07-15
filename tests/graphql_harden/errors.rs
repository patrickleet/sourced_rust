//! E* Error-leakage red-team suite.

use std::path::PathBuf;
use std::time::Duration;

use async_graphql::Request;
use distributed::graphql::{read, GraphqlEngine, ModelPermissions};

use super::common::{
    assert_no_sql_leak, engine_all_columns, error_messages, extension_code, seed_orders, session,
    OrderView,
};

/// Compile errors must not leak SQL.
#[tokio::test]
async fn compile_errors_do_not_leak_sql() {
    let pool = seed_orders().await;
    let engine = GraphqlEngine::builder(pool)
        .roles(&["user"])
        .model::<OrderView>(ModelPermissions::new().grant("user", read().all_columns()))
        .max_in_list(1)
        .build()
        .unwrap();
    let s = session("user", "x");
    let resp = engine
        .execute(
            &s,
            Request::new(r#"{ orders(where: { order_id: { _in: ["a","b"] } }) { order_id } }"#),
        )
        .await;
    assert!(!resp.errors.is_empty());
    assert_no_sql_leak(&resp);
}

/// E3: statement timeout → stable TIMEOUT code (existing harden-13 path).
#[tokio::test]
async fn e3_sqlite_statement_timeout_returns_timeout_code() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("graphql_harden_timeout_e3");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("orders.db");
    let url = format!("sqlite:{}?mode=rwc", db.display());

    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .unwrap();
    sqlx::query(
        "CREATE TABLE orders (
            order_id TEXT PRIMARY KEY,
            customer_id TEXT NOT NULL,
            status TEXT NOT NULL,
            total_cents INTEGER NOT NULL,
            note TEXT NOT NULL
        );
        INSERT INTO orders VALUES
            ('o1', 'tenant-a', 'open', 100, 'n');",
    )
    .execute(&pool)
    .await
    .unwrap();

    let mut hold = pool.acquire().await.unwrap();
    sqlx::query("BEGIN EXCLUSIVE")
        .execute(&mut *hold)
        .await
        .unwrap();

    let engine = GraphqlEngine::builder(pool.clone())
        .roles(&["user"])
        .model::<OrderView>(ModelPermissions::new().grant("user", read().all_columns()))
        .statement_timeout(Duration::from_millis(80))
        .build()
        .unwrap();

    let resp = engine
        .execute(
            &session("user", "x"),
            Request::new("{ orders { order_id } }"),
        )
        .await;

    let _ = sqlx::query("ROLLBACK").execute(&mut *hold).await;
    drop(hold);

    assert!(!resp.errors.is_empty(), "expected timeout; data={:?}", resp.data);
    let err = &resp.errors[0];
    assert!(
        err.message.to_ascii_lowercase().contains("timeout"),
        "message should mention timeout: {}",
        err.message
    );
    let code = extension_code(err);
    assert!(
        code.as_deref()
            .map(|c| c.contains("TIMEOUT"))
            .unwrap_or(false),
        "expected extensions.code=TIMEOUT, got {code:?}"
    );
    assert_no_sql_leak(&resp);
}

/// E2: execute failure (missing table) maps to INTERNAL without SQL/schema leak.
#[tokio::test]
async fn e2_execute_failure_is_internal_without_sql_leak() {
    let pool = seed_orders().await;
    let engine = engine_all_columns(pool.clone());
    // Drop table so the compiler-produced SELECT fails at execute time.
    sqlx::query("DROP TABLE orders")
        .execute(&pool)
        .await
        .unwrap();

    let s = session("user", "x");
    let resp = engine
        .execute(&s, Request::new("{ orders { order_id } }"))
        .await;
    assert!(
        !resp.errors.is_empty(),
        "expected execute error after drop table"
    );
    assert_no_sql_leak(&resp);
    for e in &resp.errors {
        let m = e.message.to_ascii_lowercase();
        assert!(
            !m.contains("no such table") && !m.contains("orders"),
            "must not leak table/schema detail: {}",
            e.message
        );
        if let Some(code) = extension_code(e) {
            assert!(
                code.contains("INTERNAL") || code.contains("BAD_REQUEST"),
                "expected INTERNAL (or safe code), got {code}"
            );
        }
    }
    // Prefer INTERNAL when extensions present.
    let codes: Vec<_> = resp
        .errors
        .iter()
        .filter_map(extension_code)
        .collect();
    if !codes.is_empty() {
        assert!(
            codes.iter().any(|c| c.contains("INTERNAL")),
            "expected at least one INTERNAL code, got {codes:?}; msgs={}",
            error_messages(&resp)
        );
    }
}
