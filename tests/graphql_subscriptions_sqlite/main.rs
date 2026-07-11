//! Phase-4 exit: subscription footprint + commit-path broadcast (SQLite).

#![cfg(all(feature = "graphql", feature = "sqlite"))]

use std::collections::BTreeSet;
use std::time::Duration;

use distributed::ReadModelChange;
use distributed::{
    ColumnType, ExpectedVersion, PrimaryKey, ReadModelWritePlanStore, RowKey, RowValue, RowValues,
    RowWriteMode, TableColumn, TableKind, TableMutation, TableRowMutation, TableSchema,
    TableWritePlan,
};
use sqlx::sqlite::SqlitePoolOptions;

#[tokio::test]
async fn broadcast_fires_on_write_plan_commit() {
    let pool = SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query(
        "CREATE TABLE items (id TEXT PRIMARY KEY, name TEXT NOT NULL, _sourced_version INTEGER NOT NULL DEFAULT 0)",
    )
    .execute(&pool)
    .await
    .unwrap();

    let repo = distributed::SqliteRepository::new(pool);
    let mut rx = repo.read_model_changes();

    let schema: &'static TableSchema = Box::leak(Box::new(TableSchema {
        model_name: "Item".into(),
        table_name: "items".into(),
        columns: vec![
            TableColumn {
                primary_key: true,
                ..TableColumn::new("id", "id", ColumnType::Text)
            },
            TableColumn::new("name", "name", ColumnType::Text),
        ],
        primary_key: PrimaryKey::new(["id"]),
        version_column: Some("_sourced_version".into()),
        foreign_keys: Vec::new(),
        indexes: Vec::new(),
        relationships: Vec::new(),
        kind: TableKind::ReadModel,
    }));

    let mut values = RowValues::new();
    values.insert("id", RowValue::String("i1".into()));
    values.insert("name", RowValue::String("one".into()));

    let plan = TableWritePlan::new(vec![TableMutation::UpsertRow(TableRowMutation {
        schema,
        key: RowKey::new([("id", RowValue::String("i1".into()))]),
        values,
        expected_version: ExpectedVersion::Any,
        mode: RowWriteMode::Upsert,
    })]);

    repo.commit_write_plan(plan).await.unwrap();

    let change = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("timeout waiting for change")
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
    // No subscribers — publish must not panic.
    repo.publish_read_model_change(ReadModelChange {
        tables: BTreeSet::from(["items".into()]),
    });
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
