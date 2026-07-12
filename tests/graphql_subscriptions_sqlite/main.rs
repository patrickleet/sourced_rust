//! Phase-4 exit: GraphQL subscription pushes exactly once per projection commit (SQLite).

#![cfg(all(feature = "graphql", feature = "sqlite"))]

use std::time::Duration;

use async_graphql::Request;
use distributed::graphql::GraphqlEngine;
use distributed::microsvc::{Session, ROLE_KEY};
use distributed::{
    ColumnType, ExpectedVersion, PrimaryKey, ReadModelChange, ReadModelWritePlanStore, RowKey,
    RowValue, RowValues, RowWriteMode, TableColumn, TableKind, TableMutation, TableRowMutation,
    TableSchema, TableWritePlan,
};
use futures_util::StreamExt;
use sqlx::sqlite::SqlitePoolOptions;

fn items_schema() -> TableSchema {
    TableSchema {
        model_name: "ItemView".into(),
        table_name: "items".into(),
        columns: vec![
            TableColumn {
                primary_key: true,
                ..TableColumn::new("id", "id", ColumnType::Text)
            },
            TableColumn::new("name", "name", ColumnType::Text),
            TableColumn::new("status", "status", ColumnType::Text),
        ],
        primary_key: PrimaryKey::new(["id"]),
        version_column: Some("_sourced_version".into()),
        foreign_keys: Vec::new(),
        indexes: Vec::new(),
        relationships: Vec::new(),
        kind: TableKind::ReadModel,
    }
}

async fn setup_fixed() -> (
    distributed::SqliteRepository,
    GraphqlEngine,
    sqlx::SqlitePool,
) {
    let pool = SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query(
        "CREATE TABLE items (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            status TEXT NOT NULL,
            _sourced_version INTEGER NOT NULL DEFAULT 0
        );
        INSERT INTO items (id, name, status, _sourced_version) VALUES
            ('i1', 'alpha', 'open', 1),
            ('i2', 'beta', 'closed', 1);",
    )
    .execute(&pool)
    .await
    .unwrap();

    let repo = distributed::SqliteRepository::new(pool.clone());
    let change_rx = repo.read_model_changes();

    let manifest =
        distributed::DistributedProjectManifest::new("items").table_schema(items_schema());
    let engine = GraphqlEngine::from_manifest(&manifest, pool.clone())
        .unwrap()
        .roles(&["user"])
        .grant_all("user")
        .change_stream(change_rx)
        .build()
        .expect("build");

    (repo, engine, pool)
}

fn user_session() -> Session {
    let mut s = Session::new();
    s.set(ROLE_KEY, "user");
    s
}

fn static_schema() -> &'static TableSchema {
    Box::leak(Box::new(items_schema()))
}

async fn upsert_item(repo: &distributed::SqliteRepository, id: &str, name: &str, status: &str) {
    let schema = static_schema();
    let mut values = RowValues::new();
    values.insert("id", RowValue::String(id.into()));
    values.insert("name", RowValue::String(name.into()));
    values.insert("status", RowValue::String(status.into()));
    let plan = TableWritePlan::new(vec![TableMutation::UpsertRow(TableRowMutation {
        schema,
        key: RowKey::new([("id", RowValue::String(id.into()))]),
        values,
        expected_version: ExpectedVersion::Any,
        mode: RowWriteMode::Upsert,
    })]);
    repo.commit_write_plan(plan).await.unwrap();
}

#[tokio::test]
async fn subscription_pushes_exactly_once_per_commit() {
    let (repo, engine, _pool) = setup_fixed().await;
    let session = user_session();

    // Filtered subscription: only open items (nested selection of fields).
    let request = Request::new(
        r#"subscription { items(where: { status: { _eq: "open" } }) { id name status } }"#,
    );
    let mut stream = Box::pin(engine.execute_stream(&session, request));

    // Initial push
    let first = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("timeout waiting initial")
        .expect("stream ended");
    assert!(!first.is_err(), "initial errors: {:?}", first.errors);
    let data = serde_json::to_value(&first.data).unwrap();
    let items = data["items"].as_array().expect("items array");
    assert_eq!(items.len(), 1, "only i1 is open: {data}");
    assert_eq!(items[0]["id"], "i1");

    // Commit a projection that changes the filtered set (i2 becomes open).
    upsert_item(&repo, "i2", "beta", "open").await;

    // Exactly one push after debounce
    let second = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("timeout waiting push")
        .expect("stream ended");
    assert!(!second.is_err(), "push errors: {:?}", second.errors);
    let data = serde_json::to_value(&second.data).unwrap();
    let items = data["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "both open after commit: {data}");

    // No third push without another commit (debounce + idle).
    let third = tokio::time::timeout(Duration::from_millis(300), stream.next()).await;
    assert!(
        third.is_err(),
        "idle subscription must not push without a commit; got {:?}",
        third
    );
}

#[tokio::test]
async fn hash_gate_no_push_when_result_unchanged() {
    let (repo, engine, _pool) = setup_fixed().await;
    let session = user_session();

    let request = Request::new(r#"subscription { items { id name status } }"#);
    let mut stream = Box::pin(engine.execute_stream(&session, request));

    let _initial = stream.next().await.expect("initial");

    // Upsert same values for i1 — projection commit fires, result unchanged → no push.
    upsert_item(&repo, "i1", "alpha", "open").await;

    let next = tokio::time::timeout(Duration::from_millis(400), stream.next()).await;
    assert!(
        next.is_err(),
        "hash gate must suppress push when payload unchanged"
    );
}

#[tokio::test]
async fn subscription_unknown_role_returns_error_response() {
    let (_repo, engine, _pool) = setup_fixed().await;
    let mut session = user_session();
    session.set(ROLE_KEY, "ghost");

    let request = Request::new(r#"subscription { items { id name status } }"#);
    let mut stream = Box::pin(engine.execute_stream(&session, request));

    let response = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("timeout waiting error")
        .expect("stream ended");
    assert!(
        response.is_err(),
        "unknown role must return an error response"
    );
    assert_eq!(
        response.errors[0].message,
        "role `ghost` is not configured for GraphQL"
    );
}

#[tokio::test]
async fn broadcast_fires_on_write_plan_commit() {
    let (repo, _engine, _pool) = setup_fixed().await;
    let mut rx = repo.read_model_changes();
    upsert_item(&repo, "i3", "gamma", "open").await;
    let change = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("timeout")
        .expect("recv");
    assert!(change.tables.contains("items"), "{change:?}");
}

#[tokio::test]
async fn zero_receiver_send_is_noop() {
    let pool = SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await
        .unwrap();
    let repo = distributed::SqliteRepository::new(pool);
    repo.publish_read_model_change(ReadModelChange::new(["items"]));
}

#[test]
fn response_hash_stable() {
    use async_graphql::Value;
    let a = Value::from(1);
    let b = Value::from(1);
    let c = Value::from(2);
    assert_eq!(
        distributed::graphql::subscribe::response_hash(&a),
        distributed::graphql::subscribe::response_hash(&b)
    );
    assert_ne!(
        distributed::graphql::subscribe::response_hash(&a),
        distributed::graphql::subscribe::response_hash(&c)
    );
}
