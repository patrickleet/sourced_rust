//! Phase-5 exit: GraphQL mutation → handler → projection → query on one endpoint.

#![cfg(all(feature = "graphql", feature = "sqlite"))]

use std::sync::Arc;

use async_graphql::Request;
use distributed::graphql::{
    exposed_command, GraphqlCommands, GraphqlEngine, GraphqlTypeDef, GraphqlTypeField,
};
use distributed::microsvc::{Context, Routes, Service, Session, ROLE_KEY};
use distributed::{
    ColumnType, ExpectedVersion, PrimaryKey, ReadModelWritePlanStore, RowKey, RowValue, RowValues,
    RowWriteMode, TableColumn, TableKind, TableMutation, TableRowMutation, TableSchema,
    TableWritePlan,
};
use serde_json::json;
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
        ],
        primary_key: PrimaryKey::new(["id"]),
        version_column: Some("_sourced_version".into()),
        foreign_keys: Vec::new(),
        indexes: Vec::new(),
        relationships: Vec::new(),
        kind: TableKind::ReadModel,
    }
}

fn static_schema() -> &'static TableSchema {
    Box::leak(Box::new(items_schema()))
}

/// Real CQRS loop: mutation dispatches handler which projects a row; query reads it.
#[tokio::test]
async fn mutation_handler_projection_query_loop() {
    let pool = SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query(
        "CREATE TABLE items (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            _sourced_version INTEGER NOT NULL DEFAULT 0
        )",
    )
    .execute(&pool)
    .await
    .unwrap();

    let repo = distributed::SqliteRepository::new(pool.clone());
    let change_rx = repo.read_model_changes();

    // Handler: write projected row via ReadModelWritePlanStore (real shipped path).
    let routes = Routes::new()
        .with_dependencies(repo)
        .command("item.create")
        .handle(|ctx: &Context<distributed::SqliteRepository>| {
            let input = ctx.raw_input().clone();
            let repo = ctx.dependencies().clone();
            async move {
                let id = input
                    .get("id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        distributed::microsvc::HandlerError::DecodeFailed("id required".into())
                    })?
                    .to_string();
                let name = input
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unnamed")
                    .to_string();

                let schema = static_schema();
                let mut values = RowValues::new();
                values.insert("id", RowValue::String(id.clone()));
                values.insert("name", RowValue::String(name));
                let plan = TableWritePlan::new(vec![TableMutation::UpsertRow(TableRowMutation {
                    schema,
                    key: RowKey::new([("id", RowValue::String(id.clone()))]),
                    values,
                    expected_version: ExpectedVersion::Any,
                    mode: RowWriteMode::Upsert,
                })]);
                repo.commit_write_plan(plan).await.map_err(|e| {
                    distributed::microsvc::HandlerError::from(distributed::RepositoryError::from(e))
                })?;
                Ok(json!({ "id": id }))
            }
        });
    let service = Arc::new(Service::new().routes(routes));

    let commands = GraphqlCommands::new().command(
        "item.create",
        exposed_command()
            .field_name("create_item")
            .input_json()
            .roles(["user"]),
    );

    let manifest =
        distributed::DistributedProjectManifest::new("items").table_schema(items_schema());
    let engine = GraphqlEngine::from_manifest(&manifest, pool)
        .unwrap()
        .roles(&["user", "anonymous"])
        .grant_all("user")
        .commands(commands)
        .change_stream(change_rx)
        .build()
        .expect("build");

    let mut session = Session::new();
    session.set(ROLE_KEY, "user");

    // 1) Mutation dispatches real handler → projects row
    let mut_req =
        Request::new(r#"mutation { create_item(input: { id: "item-1", name: "widget" }) }"#)
            .data(Arc::clone(&service));
    let mut_resp = engine.execute(&session, mut_req).await;
    assert!(
        !mut_resp.is_err(),
        "mutation must succeed: {:?}",
        mut_resp.errors
    );
    let mut_data = serde_json::to_value(&mut_resp.data).unwrap();
    assert_eq!(
        mut_data["create_item"]["id"], "item-1",
        "mutation payload: {mut_data}"
    );

    // 2) Query reads projected row on the same GraphQL endpoint/engine
    let q_resp = engine
        .execute(&session, Request::new(r#"{ items { id name } }"#))
        .await;
    assert!(!q_resp.is_err(), "query must succeed: {:?}", q_resp.errors);
    let q_data = serde_json::to_value(&q_resp.data).unwrap();
    let items = q_data["items"].as_array().expect("items");
    assert_eq!(items.len(), 1, "projected row visible: {q_data}");
    assert_eq!(items[0]["id"], "item-1");
    assert_eq!(items[0]["name"], "widget");
}

#[tokio::test]
async fn no_mutation_root_for_role_without_commands() {
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

    let commands = GraphqlCommands::new().command(
        "item.create",
        exposed_command().input_json().roles(["user"]),
    );
    let manifest =
        distributed::DistributedProjectManifest::new("items").table_schema(items_schema());
    let engine = GraphqlEngine::from_manifest(&manifest, pool)
        .unwrap()
        .roles(&["user", "anonymous"])
        .grant_all("user")
        .commands(commands)
        .build()
        .unwrap();

    let user_sdl = engine.sdl_for_role("user").expect("user schema");
    assert!(
        user_sdl.contains("type Mutation"),
        "user role with commands must expose Mutation root: {user_sdl}"
    );

    let anon_sdl = engine.sdl_for_role("anonymous").expect("anonymous schema");
    // Anonymous has no commands → no Mutation type in SDL.
    assert!(
        !anon_sdl.contains("type Mutation"),
        "anonymous role must not expose Mutation root: {anon_sdl}"
    );
}

#[test]
#[should_panic(expected = "command `item.create` is already registered")]
fn duplicate_command_names_panic_with_api_error() {
    let _ = GraphqlCommands::new()
        .command("item.create", exposed_command().input_json())
        .command("item.create", exposed_command().input_json());
}

#[tokio::test]
async fn standalone_router_without_service_returns_no_dispatcher() {
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

    let commands = GraphqlCommands::new().command(
        "item.create",
        exposed_command()
            .field_name("create_item")
            .input_json()
            .roles(["user"]),
    );
    let manifest =
        distributed::DistributedProjectManifest::new("items").table_schema(items_schema());
    let engine = GraphqlEngine::from_manifest(&manifest, pool)
        .unwrap()
        .roles(&["user"])
        .grant_all("user")
        .commands(commands)
        .build()
        .unwrap();

    let mut session = Session::new();
    session.set(ROLE_KEY, "user");
    // No Service in request data → INTERNAL no-dispatcher path.
    let resp = engine
        .execute(
            &session,
            Request::new(r#"mutation { create_item(input: { id: "x" }) }"#),
        )
        .await;
    assert!(resp.is_err(), "must error without dispatcher");
    let err = format!("{:?}", resp.errors);
    assert!(
        err.contains("dispatcher not configured")
            || err.contains("INTERNAL")
            || err.contains("not configured"),
        "expected no-dispatcher error, got {err}"
    );
    let code = resp.errors[0]
        .extensions
        .as_ref()
        .and_then(|ext| ext.get("code"))
        .map(|v| format!("{v}"));
    assert!(
        code.as_deref().is_some_and(|c| c.contains("INTERNAL")),
        "no-dispatcher must set extensions.code=INTERNAL, got {code:?}"
    );
}

/// Empty-role schema returns FORBIDDEN with extensions.code (frozen contract).
#[tokio::test]
async fn empty_role_forbidden_sets_extensions_code() {
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

    let manifest =
        distributed::DistributedProjectManifest::new("items").table_schema(items_schema());
    // Role "nobody" is declared but never granted any model → empty grant surface.
    let engine = GraphqlEngine::from_manifest(&manifest, pool)
        .unwrap()
        .roles(&["user", "nobody"])
        .grant_all("user")
        .build()
        .expect("build");

    let mut session = Session::new();
    session.set(ROLE_KEY, "nobody");
    let resp = engine
        .execute(&session, Request::new("{ _empty }"))
        .await;
    assert!(resp.is_err(), "empty role must error");
    let err = &resp.errors[0];
    assert!(
        err.message.contains("no GraphQL grants") || err.message.contains("FORBIDDEN"),
        "message: {}",
        err.message
    );
    let code = err
        .extensions
        .as_ref()
        .and_then(|ext| ext.get("code"))
        .map(|v| format!("{v}"));
    assert!(
        code.as_deref().is_some_and(|c| c.contains("FORBIDDEN")),
        "empty role must set extensions.code=FORBIDDEN, got {code:?} err={err:?}"
    );
}

/// Command mutation maps handler HTTP status → extensions.code (+ status).
#[tokio::test]
async fn command_mutation_errors_set_extensions_code_and_status() {
    use distributed::microsvc::HandlerError;

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

    async fn assert_code(
        engine: &GraphqlEngine,
        service: &Arc<Service>,
        session: &Session,
        status_hint: &str,
        expect_code: &str,
        expect_status: i64,
    ) {
        let req = Request::new(format!(
            r#"mutation {{ fail_cmd(input: {{ want: "{status_hint}" }}) }}"#
        ))
        .data(Arc::clone(service));
        let resp = engine.execute(session, req).await;
        assert!(
            resp.is_err(),
            "expected error for {status_hint}, got {:?}",
            resp.data
        );
        let err = &resp.errors[0];
        let ext = err.extensions.as_ref().expect("extensions present");
        let code = ext.get("code").map(|v| format!("{v}"));
        let status = ext.get("status").map(|v| format!("{v}"));
        assert!(
            code.as_deref().is_some_and(|c| c.contains(expect_code)),
            "want extensions.code={expect_code} for {status_hint}, got code={code:?} msg={}",
            err.message
        );
        assert!(
            status
                .as_deref()
                .is_some_and(|s| s.contains(&expect_status.to_string())),
            "want extensions.status={expect_status} for {status_hint}, got {status:?}"
        );
        // Message must not embed [CODE] bracket form (legacy).
        assert!(
            !err.message.contains(&format!("[{expect_code}]")),
            "legacy [CODE] in message: {}",
            err.message
        );
    }

    let routes = Routes::new()
        .command("item.fail")
        .handle(|ctx: &Context<()>| {
            let want = ctx
                .raw_input()
                .get("want")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            async move {
                match want.as_str() {
                    "401" => Err(HandlerError::Unauthorized("nope".into())),
                    "404" => Err(HandlerError::NotFound("missing".into())),
                    "422" => Err(HandlerError::Rejected("invalid".into())),
                    "400" => Err(HandlerError::DecodeFailed("bad".into())),
                    "500" => Err(HandlerError::Other(Box::new(std::io::Error::other("boom")))),
                    _ => Ok(json!({ "ok": true })),
                }
            }
        });
    let service = Arc::new(Service::new().routes(routes));

    let commands = GraphqlCommands::new().command(
        "item.fail",
        exposed_command()
            .field_name("fail_cmd")
            .input_json()
            .roles(["user"]),
    );
    let manifest =
        distributed::DistributedProjectManifest::new("items").table_schema(items_schema());
    let engine = GraphqlEngine::from_manifest(&manifest, pool)
        .unwrap()
        .roles(&["user"])
        .grant_all("user")
        .commands(commands)
        .build()
        .expect("build");

    let mut session = Session::new();
    session.set(ROLE_KEY, "user");

    assert_code(&engine, &service, &session, "401", "UNAUTHORIZED", 401).await;
    assert_code(&engine, &service, &session, "404", "NOT_FOUND", 404).await;
    assert_code(&engine, &service, &session, "422", "REJECTED", 422).await;
    assert_code(&engine, &service, &session, "400", "BAD_REQUEST", 400).await;
    assert_code(&engine, &service, &session, "500", "INTERNAL", 500).await;
}

#[test]
fn graphql_type_def_mapping_golden() {
    // Manual GraphqlTypeDef shapes used by derives / exposed_command.
    let input = GraphqlTypeDef::new(
        "CreateItemInput",
        vec![
            GraphqlTypeField {
                name: "id".into(),
                type_name: "String".into(),
                nullable: false,
                list: false,
                nested: None,
            },
            GraphqlTypeField {
                name: "tags".into(),
                type_name: "String".into(),
                nullable: true,
                list: true,
                nested: None,
            },
        ],
    );
    assert_eq!(input.name, "CreateItemInput");
    assert_eq!(input.fields.len(), 2);
    assert!(!input.fields[0].nullable);
    assert!(input.fields[1].list);
}

#[derive(distributed::GraphqlInput)]
#[allow(dead_code)]
struct DerivedInput {
    id: String,
    count: i64,
    tags: Option<Vec<String>>,
}

#[derive(distributed::GraphqlOutput)]
#[allow(dead_code)]
struct DerivedOutput {
    ok: bool,
    id: String,
}

#[test]
fn derive_mapping_golden() {
    use distributed::graphql::{GraphqlInputType, GraphqlOutputType};
    let input = DerivedInput::graphql_type();
    assert_eq!(input.name, "DerivedInput");
    assert_eq!(input.fields.len(), 3);
    assert_eq!(input.fields[0].type_name, "String");
    assert!(!input.fields[0].nullable);
    assert_eq!(input.fields[1].type_name, "BigInt");
    assert!(input.fields[2].list);
    assert!(input.fields[2].nullable);

    let output = DerivedOutput::graphql_type();
    assert_eq!(output.name, "DerivedOutput");
    assert_eq!(output.fields[0].type_name, "Boolean");
    assert_eq!(output.fields[1].type_name, "String");
}
