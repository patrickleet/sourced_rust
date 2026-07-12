//! Shared fixtures for GraphQL harden / red-team suites.
//! All tests drive shipped `GraphqlEngine` paths only.

use async_graphql::Request;
use distributed::graphql::{select, GraphqlEngine, ModelPermissions};
use distributed::microsvc::{Session, ROLE_KEY, USER_ID_KEY};
use distributed::ReadModel;
use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqlitePoolOptions;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ReadModel)]
#[table("orders")]
pub struct OrderView {
    #[id("order_id")]
    pub order_id: String,
    pub customer_id: String,
    pub status: String,
    pub total_cents: i64,
    /// May look like JSON; must remain a GraphQL String.
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ReadModel)]
#[table("parents")]
pub struct ParentView {
    #[id("parent_id")]
    pub parent_id: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ReadModel)]
#[table("children")]
pub struct ChildView {
    #[id("child_id")]
    pub child_id: String,
    pub parent_id: String,
    pub name: String,
}

pub fn session(role: &str, user: &str) -> Session {
    let mut s = Session::new();
    s.set(ROLE_KEY, role);
    s.set(USER_ID_KEY, user);
    s
}

pub async fn seed_orders() -> sqlx::SqlitePool {
    let pool = SqlitePoolOptions::new()
        .connect("sqlite::memory:")
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
            ('o1', 'tenant-a', 'open', 100, '{\"looks\":\"json\"}'),
            ('o2', 'tenant-a', 'shipped', 200, 'plain'),
            ('o3', 'tenant-b', 'open', 50, 'x');",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool
}

pub fn engine_all_columns(pool: sqlx::SqlitePool) -> GraphqlEngine {
    GraphqlEngine::builder(pool)
        .roles(&["user"])
        .model::<OrderView>(ModelPermissions::new().role("user", select().all_columns()))
        .build()
        .unwrap()
}

pub fn assert_no_sql_leak(resp: &async_graphql::Response) {
    for e in &resp.errors {
        let m = e.message.to_ascii_lowercase();
        assert!(
            !m.contains("select ") && !m.contains(" from ") && !m.contains(" where "),
            "must not leak SQL: {}",
            e.message
        );
    }
}

pub fn error_messages(resp: &async_graphql::Response) -> String {
    resp.errors
        .iter()
        .map(|e| e.message.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn extension_code(err: &async_graphql::ServerError) -> Option<String> {
    err.extensions
        .as_ref()
        .and_then(|ext| ext.get("code"))
        .map(|v| format!("{v:?}"))
}

/// Helper: execute and return JSON data (panics if GraphQL errors when `expect_ok`).
pub async fn exec_json(
    engine: &GraphqlEngine,
    session: &Session,
    query: &str,
) -> serde_json::Value {
    let resp = engine.execute(session, Request::new(query)).await;
    assert!(
        resp.errors.is_empty(),
        "unexpected errors: {:?}",
        resp.errors
    );
    serde_json::to_value(&resp.data).unwrap()
}
