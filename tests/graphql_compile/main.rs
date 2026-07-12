//! Production compile-path behavioral goldens (via GraphqlEngine::execute).
//! Empty placeholder removed — suite drives shipped compile_root through execute.

#![cfg(all(feature = "graphql", feature = "sqlite"))]

use async_graphql::Request;
use distributed::graphql::{select, GraphqlEngine, ModelPermissions};
use distributed::microsvc::{Session, ROLE_KEY};
use distributed::ReadModel;
use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqlitePoolOptions;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ReadModel)]
#[table("items")]
struct ItemView {
    #[id("id")]
    id: String,
    name: String,
}

fn user() -> Session {
    let mut s = Session::new();
    s.set(ROLE_KEY, "user");
    s
}

async fn engine() -> GraphqlEngine {
    let pool = SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query("CREATE TABLE items (id TEXT PRIMARY KEY, name TEXT NOT NULL)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO items VALUES ('1', 'a'), ('2', 'b'), ('3', 'c')")
        .execute(&pool)
        .await
        .unwrap();
    GraphqlEngine::builder(pool)
        .roles(&["user"])
        .model::<ItemView>(ModelPermissions::new().role("user", select().all_columns()))
        .build()
        .unwrap()
}

#[tokio::test]
async fn list_and_by_pk_compile_and_execute() {
    let engine = engine().await;
    let resp = engine
        .execute(&user(), Request::new("{ items { id name } }"))
        .await;
    assert!(!resp.is_err(), "{:?}", resp.errors);
    let data = serde_json::to_value(&resp.data).unwrap();
    assert_eq!(data["items"].as_array().unwrap().len(), 3);

    let resp = engine
        .execute(
            &user(),
            Request::new(r#"{ items_by_pk(id: "2") { name } }"#),
        )
        .await;
    assert!(!resp.is_err(), "{:?}", resp.errors);
    let data = serde_json::to_value(&resp.data).unwrap();
    assert_eq!(data["items_by_pk"]["name"], "b");
}

#[tokio::test]
async fn where_eq_uses_production_compiler() {
    let engine = engine().await;
    let resp = engine
        .execute(
            &user(),
            Request::new(r#"{ items(where: { name: { _eq: "a" } }) { id } }"#),
        )
        .await;
    assert!(!resp.is_err(), "{:?}", resp.errors);
    let data = serde_json::to_value(&resp.data).unwrap();
    assert_eq!(data["items"].as_array().unwrap().len(), 1);
    assert_eq!(data["items"][0]["id"], "1");
}
