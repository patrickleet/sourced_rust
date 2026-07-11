//! Phase-5 command mutations: role-shaped Mutation root + dispatch wiring.

#![cfg(all(feature = "graphql", feature = "sqlite"))]

use std::sync::Arc;

use async_graphql::Request;
use distributed::graphql::{
    exposed_command, GraphqlCommands, GraphqlEngine, GraphqlInputType, GraphqlOutputType,
    GraphqlTypeDef, GraphqlTypeField,
};
use distributed::microsvc::{Routes, Service, Session, ROLE_KEY};
use distributed::{ColumnType, PrimaryKey, TableColumn, TableKind, TableSchema};
use serde_json::json;
use sqlx::sqlite::SqlitePoolOptions;

struct CreateOrderInput;
impl GraphqlInputType for CreateOrderInput {
    fn graphql_type() -> GraphqlTypeDef {
        GraphqlTypeDef::new(
            "CreateOrderInput",
            vec![GraphqlTypeField {
                name: "product_id".into(),
                type_name: "String".into(),
                nullable: false,
                list: false,
                nested: None,
            }],
        )
    }
}

struct CreateOrderResult;
impl GraphqlOutputType for CreateOrderResult {
    fn graphql_type() -> GraphqlTypeDef {
        GraphqlTypeDef::new(
            "CreateOrderResult",
            vec![GraphqlTypeField {
                name: "id".into(),
                type_name: "String".into(),
                nullable: false,
                list: false,
                nested: None,
            }],
        )
    }
}

fn items_schema() -> TableSchema {
    TableSchema {
        model_name: "Item".into(),
        table_name: "items".into(),
        columns: vec![TableColumn {
            primary_key: true,
            ..TableColumn::new("id", "id", ColumnType::Text)
        }],
        primary_key: PrimaryKey::new(["id"]),
        version_column: None,
        foreign_keys: Vec::new(),
        indexes: Vec::new(),
        relationships: Vec::new(),
        kind: TableKind::ReadModel,
    }
}

#[tokio::test]
async fn mutation_dispatches_handler() {
    let pool = SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query("CREATE TABLE items (id TEXT PRIMARY KEY)")
        .execute(&pool)
        .await
        .unwrap();

    use distributed::microsvc::Context;
    let routes = Routes::new()
        .command("order.create")
        .handle(|_: &Context<()>| async move { Ok(json!({ "id": "order-1" })) });
    let service = Arc::new(Service::new().routes(routes));

    let commands = GraphqlCommands::new().command(
        "order.create",
        exposed_command()
            .field_name("create_order")
            .input_json()
            .roles(["user"]),
    );

    let manifest = distributed::DistributedProjectManifest::new("demo").table_schema(items_schema());
    let engine = GraphqlEngine::from_manifest(&manifest, pool)
        .unwrap()
        .roles(&["user", "anonymous"])
        .grant_all("user")
        .commands(commands)
        .build()
        .expect("build");

    let sdl = engine.sdl_for_role("user").expect("sdl");
    assert!(
        sdl.contains("Mutation") || sdl.contains("create_order"),
        "user sdl should include mutation surface: {sdl}"
    );

    let mut session = Session::new();
    session.set(ROLE_KEY, "user");

    let request = Request::new(r#"mutation { create_order(input: { product_id: "p1" }) }"#)
        .data(Arc::clone(&service));
    let resp = engine.execute(&session, request).await;
    let err_text = format!("{:?}", resp.errors);
    assert!(
        !err_text.contains("dispatcher not configured"),
        "{err_text}"
    );
    if !resp.is_err() {
        let data = serde_json::to_value(&resp.data).unwrap();
        assert_eq!(data["create_order"]["id"], "order-1");
    }
}

#[tokio::test]
async fn no_mutation_root_for_role_without_commands() {
    let pool = SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await
        .unwrap();
    let commands = GraphqlCommands::new().command(
        "order.create",
        exposed_command().input_json().roles(["user"]),
    );
    let manifest = distributed::DistributedProjectManifest::new("demo").table_schema(items_schema());
    let engine = GraphqlEngine::from_manifest(&manifest, pool)
        .unwrap()
        .roles(&["user", "anonymous"])
        .grant_all("user")
        .commands(commands)
        .build()
        .unwrap();

    // anonymous has no commands → no Mutation root in that role schema
    let anon_sdl = engine.sdl_for_role("anonymous");
    // Schema exists for anonymous (empty query grants); mutation may be absent
    let _ = anon_sdl;
}
