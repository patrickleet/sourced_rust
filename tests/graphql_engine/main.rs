//! GraphqlEngine builder validation tests.

#![cfg(all(feature = "graphql", feature = "sqlite"))]

use distributed::{
    graphql::{col, claim, select, GraphqlEngine},
    ColumnType, PrimaryKey, TableColumn, TableKind, TableSchema,
};
use sqlx::sqlite::SqlitePoolOptions;

fn simple_schema(model: &str, table: &str) -> TableSchema {
    TableSchema {
        model_name: model.into(),
        table_name: table.into(),
        columns: vec![
            TableColumn {
                primary_key: true,
                ..TableColumn::new("id", "id", ColumnType::Text)
            },
            TableColumn::new("owner_id", "owner_id", ColumnType::Text),
        ],
        primary_key: PrimaryKey::new(["id"]),
        version_column: None,
        foreign_keys: Vec::new(),
        indexes: Vec::new(),
        relationships: Vec::new(),
        kind: TableKind::ReadModel,
    }
}

async fn pool() -> sqlx::SqlitePool {
    SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await
        .unwrap()
}

#[tokio::test]
async fn grant_all_builds_and_sdl_for_role() {
    let schema = simple_schema("Item", "items");
    let manifest = distributed::DistributedProjectManifest::new("t").table_schema(schema);
    let engine = GraphqlEngine::from_manifest(&manifest, pool().await)
        .unwrap()
        .roles(&["user", "anonymous"])
        .grant_all("user")
        .build()
        .unwrap();
    let sdl = engine.sdl_for_role("user").expect("role sdl");
    assert!(sdl.contains("items") || sdl.contains("Item"));
}

#[tokio::test]
async fn duplicate_table_name_errors() {
    let a = simple_schema("A", "items");
    let b = simple_schema("B", "items");
    let result = GraphqlEngine::builder(pool().await)
        .table_schema(a)
        .table_schema(b)
        .build();
    let err = match result {
        Ok(_) => panic!("expected duplicate table_name error"),
        Err(e) => e,
    };
    assert!(
        err.to_string().contains("duplicate table_name") || err.to_string().contains("items"),
        "{err}"
    );
}

#[tokio::test]
async fn unknown_column_in_permission_via_grant_ok() {
    // grant_all uses all_columns — valid
    let schema = simple_schema("Item", "items");
    let manifest = distributed::DistributedProjectManifest::new("t").table_schema(schema);
    GraphqlEngine::from_manifest(&manifest, pool().await)
        .unwrap()
        .grant_all("user")
        .build()
        .unwrap();
}

#[tokio::test]
async fn pure_sql_compile_helper() {
    use distributed::graphql::naming::root_list_field;
    let schema = simple_schema("Item", "items");
    assert_eq!(root_list_field(&schema), "items");
    let sql = distributed::graphql::sdl::graphql_sdl_for_tables(&[schema]).unwrap();
    assert!(sql.contains("type Item"));
}
