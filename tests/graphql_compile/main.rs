//! Production compile-path behavioral goldens (via GraphqlEngine::execute).
//! Empty placeholder removed — suite drives shipped compile_root through execute.

#![cfg(all(feature = "graphql", feature = "sqlite"))]

use async_graphql::Request;
use distributed::graphql::{read, GraphqlEngine, ModelPermissions};
use distributed::microsvc::{Session, ROLE_KEY};
use distributed::ReadModel;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
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
        .model::<ItemView>(ModelPermissions::new().grant("user", read().all_columns()))
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ReadModel)]
#[table("client_plan_rows")]
struct ClientPlanRow {
    #[id("id")]
    id: String,
    priority: i32,
    completed: bool,
}

#[derive(Deserialize)]
struct ClientPlanCorpus {
    records: Vec<ClientPlanRow>,
    cases: Vec<ClientPlanCase>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClientPlanCase {
    name: String,
    #[serde(rename = "where")]
    where_: JsonValue,
    order_by: JsonValue,
    limit: i32,
    offset: i32,
    expected: Vec<String>,
}

fn graphql_input(value: &JsonValue) -> String {
    match value {
        JsonValue::Null => "null".into(),
        JsonValue::Bool(value) => value.to_string(),
        JsonValue::Number(value) => value.to_string(),
        JsonValue::String(value) => serde_json::to_string(value).expect("serialize string"),
        JsonValue::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(graphql_input)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        JsonValue::Object(values) => format!(
            "{{{}}}",
            values
                .iter()
                .map(|(name, value)| format!("{name}: {}", graphql_input(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn graphql_order(value: &JsonValue) -> String {
    let entries = value.as_array().expect("orderBy array");
    format!(
        "[{}]",
        entries
            .iter()
            .map(|entry| {
                let object = entry.as_object().expect("orderBy object");
                let (field, direction) = object.iter().next().expect("orderBy field");
                assert_eq!(object.len(), 1, "orderBy entry must be unambiguous");
                format!(
                    "{{{field}: {}}}",
                    direction.as_str().expect("orderBy direction")
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    )
}

#[tokio::test]
async fn portable_query_plan_corpus_matches_sqlite_server_semantics() {
    let corpus: ClientPlanCorpus =
        serde_json::from_str(include_str!("../fixtures/client-query-plan-corpus.json"))
            .expect("parse shared client query-plan corpus");
    let pool = SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query(
        "CREATE TABLE client_plan_rows (id TEXT PRIMARY KEY, priority INTEGER NOT NULL, completed INTEGER NOT NULL)",
    )
    .execute(&pool)
    .await
    .unwrap();
    for record in &corpus.records {
        sqlx::query("INSERT INTO client_plan_rows (id, priority, completed) VALUES (?, ?, ?)")
            .bind(&record.id)
            .bind(record.priority)
            .bind(record.completed)
            .execute(&pool)
            .await
            .unwrap();
    }
    let engine = GraphqlEngine::builder(pool)
        .roles(&["user"])
        .model::<ClientPlanRow>(ModelPermissions::new().grant("user", read().all_columns()))
        .build()
        .unwrap();

    for case in corpus.cases {
        let query = format!(
            "{{ client_plan_rows(where: {}, order_by: {}, limit: {}, offset: {}) {{ id }} }}",
            graphql_input(&case.where_),
            graphql_order(&case.order_by),
            case.limit,
            case.offset
        );
        let response = engine.execute(&user(), Request::new(query)).await;
        assert!(!response.is_err(), "{}: {:?}", case.name, response.errors);
        let data = serde_json::to_value(response.data).unwrap();
        let actual = data["client_plan_rows"]
            .as_array()
            .expect("rows")
            .iter()
            .map(|row| row["id"].as_str().expect("string id").to_string())
            .collect::<Vec<_>>();
        assert_eq!(actual, case.expected, "{}", case.name);
    }
}
