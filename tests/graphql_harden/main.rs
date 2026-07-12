//! Harden suite: security, AuthZ, relationships, metrics, limits.
//! Drives shipped `GraphqlEngine::execute` (real compile + execute path).

#![cfg(all(feature = "graphql", feature = "sqlite"))]

use async_graphql::Request;
use distributed::graphql::{
    claim, col, select, GraphqlEngine, ModelPermissions,
};
use distributed::microsvc::{Session, ROLE_KEY, USER_ID_KEY};
use distributed::{ReadModel, RelationalReadModel};
use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqlitePoolOptions;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ReadModel)]
#[table("orders")]
struct OrderView {
    #[id("order_id")]
    order_id: String,
    customer_id: String,
    status: String,
    total_cents: i64,
    /// May look like JSON; must remain a GraphQL String.
    note: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ReadModel)]
#[table("parents")]
struct ParentView {
    #[id("parent_id")]
    parent_id: String,
    name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ReadModel)]
#[table("children")]
struct ChildView {
    #[id("child_id")]
    child_id: String,
    parent_id: String,
    name: String,
}

fn session(role: &str, user: &str) -> Session {
    let mut s = Session::new();
    s.set(ROLE_KEY, role);
    s.set(USER_ID_KEY, user);
    s
}

async fn seed_orders() -> sqlx::SqlitePool {
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

#[tokio::test]
async fn claim_row_filter_isolates_tenants() {
    let pool = seed_orders().await;
    let engine = GraphqlEngine::builder(pool)
        .roles(&["user", "anonymous"])
        .model::<OrderView>(
            ModelPermissions::new().role(
                "user",
                select()
                    .all_columns()
                    .filter(col("customer_id").eq(claim("x-user-id"))),
            ),
        )
        .build()
        .unwrap();

    let a = session("user", "tenant-a");
    let resp = engine
        .execute(&a, Request::new("{ orders { order_id customer_id } }"))
        .await;
    assert!(!resp.is_err(), "{:?}", resp.errors);
    let data = serde_json::to_value(&resp.data).unwrap();
    let orders = data["orders"].as_array().unwrap();
    assert_eq!(orders.len(), 2);
    assert!(orders.iter().all(|o| o["customer_id"] == "tenant-a"));

    let b = session("user", "tenant-b");
    let resp = engine
        .execute(&b, Request::new("{ orders { order_id } }"))
        .await;
    let data = serde_json::to_value(&resp.data).unwrap();
    assert_eq!(data["orders"].as_array().unwrap().len(), 1);

    // by_pk cross-tenant → null
    let resp = engine
        .execute(
            &b,
            Request::new(r#"{ orders_by_pk(order_id: "o1") { order_id } }"#),
        )
        .await;
    let data = serde_json::to_value(&resp.data).unwrap();
    assert!(data["orders_by_pk"].is_null() || data.get("orders_by_pk").is_none());
}

#[tokio::test]
async fn json_looking_string_column_stays_string() {
    let pool = seed_orders().await;
    let engine = GraphqlEngine::builder(pool)
        .roles(&["user"])
        .model::<OrderView>(ModelPermissions::new().role("user", select().all_columns()))
        .build()
        .unwrap();
    let s = session("user", "tenant-a");
    let resp = engine
        .execute(
            &s,
            Request::new(r#"{ orders_by_pk(order_id: "o1") { note } }"#),
        )
        .await;
    assert!(!resp.is_err(), "{:?}", resp.errors);
    let data = serde_json::to_value(&resp.data).unwrap();
    assert!(
        data["orders_by_pk"]["note"].is_string(),
        "note must remain string, got {:?}",
        data["orders_by_pk"]["note"]
    );
    assert_eq!(data["orders_by_pk"]["note"], "{\"looks\":\"json\"}");
}

#[tokio::test]
async fn max_in_list_rejected() {
    let pool = seed_orders().await;
    let engine = GraphqlEngine::builder(pool)
        .roles(&["user"])
        .model::<OrderView>(ModelPermissions::new().role("user", select().all_columns()))
        .max_in_list(3)
        .build()
        .unwrap();
    let s = session("user", "x");
    // 4 ids > max_in_list 3
    let q = r#"{ orders(where: { order_id: { _in: ["a","b","c","d"] } }) { order_id } }"#;
    let resp = engine.execute(&s, Request::new(q)).await;
    assert!(
        resp.is_err() || !resp.errors.is_empty(),
        "expected list-too-long style error"
    );
}

#[tokio::test]
async fn limit_clamped_by_max_limit() {
    let pool = seed_orders().await;
    let engine = GraphqlEngine::builder(pool)
        .roles(&["user"])
        .model::<OrderView>(ModelPermissions::new().role("user", select().all_columns()))
        .max_limit(1)
        .default_limit(1)
        .build()
        .unwrap();
    let s = session("user", "x");
    let resp = engine
        .execute(&s, Request::new("{ orders(limit: 100) { order_id } }"))
        .await;
    assert!(!resp.is_err(), "{:?}", resp.errors);
    let data = serde_json::to_value(&resp.data).unwrap();
    assert_eq!(data["orders"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn nested_has_many_relationship_e2e() {
    let pool = SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query(
        "CREATE TABLE parents (parent_id TEXT PRIMARY KEY, name TEXT NOT NULL);
         CREATE TABLE children (
            child_id TEXT PRIMARY KEY,
            parent_id TEXT NOT NULL,
            name TEXT NOT NULL
         );
         INSERT INTO parents VALUES ('p1', 'P');
         INSERT INTO children VALUES ('c1', 'p1', 'C1'), ('c2', 'p1', 'C2');",
    )
    .execute(&pool)
    .await
    .unwrap();

    // Register via manifest so tables are exposed; attach HasMany metadata.
    let mut parent = ParentView::schema().clone();
    parent.relationships = vec![distributed::RelationshipDef {
        field_name: "children".into(),
        kind: distributed::RelationshipKind::HasMany,
        target_model: "ChildView".into(),
        foreign_key: Some("parent_id".into()),
        through: None,
        target_foreign_key: None,
    }];
    let child = ChildView::schema().clone();
    let manifest = distributed::DistributedProjectManifest::new("rel")
        .table_schema(parent)
        .table_schema(child);

    let engine = GraphqlEngine::from_manifest(&manifest, pool)
        .unwrap()
        .roles(&["user"])
        .grant_all("user")
        .build()
        .expect("build");

    let s = session("user", "u");
    let resp = engine
        .execute(
            &s,
            Request::new("{ parents { parent_id children { child_id name } } }"),
        )
        .await;
    assert!(!resp.is_err(), "{:?}", resp.errors);
    let data = serde_json::to_value(&resp.data).unwrap();
    let children = data["parents"][0]["children"].as_array().unwrap();
    assert_eq!(children.len(), 2);
}

#[tokio::test]
async fn compile_errors_do_not_leak_sql() {
    let pool = seed_orders().await;
    let engine = GraphqlEngine::builder(pool)
        .roles(&["user"])
        .model::<OrderView>(ModelPermissions::new().role("user", select().all_columns()))
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
    let msgs: Vec<String> = resp.errors.iter().map(|e| e.message.clone()).collect();
    for m in &msgs {
        assert!(
            !m.to_ascii_lowercase().contains("select"),
            "leaked SQL-ish message: {m}"
        );
    }
}

#[cfg(feature = "metrics")]
#[tokio::test]
async fn graphql_metrics_increment_on_execute() {
    let pool = seed_orders().await;
    let engine = GraphqlEngine::builder(pool)
        .roles(&["user"])
        .model::<OrderView>(ModelPermissions::new().role("user", select().all_columns()))
        .build()
        .unwrap();
    let s = session("user", "x");
    let _ = engine
        .execute(&s, Request::new("{ orders { order_id } }"))
        .await;
    let text = distributed::metrics::prometheus_text();
    assert!(
        text.contains("distributed_graphql_request_total"),
        "metrics text missing graphql series: {text}"
    );
}
