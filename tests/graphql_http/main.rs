//! HTTP GraphiQL on/off + session role integration tests.

#![cfg(all(feature = "graphql", feature = "sqlite"))]

use std::sync::Arc;

use distributed::graphql::{select, GraphqlEngine, ModelPermissions};
use distributed::microsvc::{router, Service};
use distributed::{ReadModel, RelationalReadModel};
use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqlitePoolOptions;
use tower::util::ServiceExt;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ReadModel)]
#[table("http_items")]
struct HttpItem {
    #[id("id")]
    id: String,
    name: String,
}

async fn service_with_graphiql(on: bool) -> Arc<Service> {
    let pool = SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query(
        "CREATE TABLE http_items (id TEXT PRIMARY KEY, name TEXT NOT NULL);
         INSERT INTO http_items VALUES ('1', 'n');",
    )
    .execute(&pool)
    .await
    .unwrap();
    let engine = GraphqlEngine::builder(pool)
        .roles(&["user"])
        .model::<HttpItem>(ModelPermissions::new().role("user", select().all_columns()))
        .graphiql(on)
        .build()
        .unwrap();
    Arc::new(Service::new().named("http-gql").with_graphql(engine))
}

#[tokio::test]
async fn graphiql_get_200_when_enabled() {
    let svc = service_with_graphiql(true).await;
    let app = router(svc);
    let res = app
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/graphql")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::OK);
    let bytes = axum::body::to_bytes(res.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body = String::from_utf8_lossy(&bytes);
    assert!(
        body.contains("GraphiQL") || body.contains("graphiql") || body.contains("/graphql"),
        "unexpected body: {body}"
    );
}

#[tokio::test]
async fn graphiql_get_405_when_disabled() {
    let svc = service_with_graphiql(false).await;
    let app = router(svc);
    let res = app
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/graphql")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn post_graphql_with_role_returns_data() {
    let svc = service_with_graphiql(false).await;
    let app = router(svc);
    let res = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/graphql")
                .header("content-type", "application/json")
                .header("x-role", "user")
                .body(axum::body::Body::from(
                    r#"{"query":"{ http_items { id name } }"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::OK);
    let bytes = axum::body::to_bytes(res.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(
        v["data"]["http_items"].as_array().map(|a| !a.is_empty()).unwrap_or(false),
        "response: {v}"
    );
}
