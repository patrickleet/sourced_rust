//! A* Authorization red-team + related AuthZ e2e.
//! Drives shipped `GraphqlEngine::execute`.

use async_graphql::Request;
use distributed::graphql::{claim, col, select, GraphqlEngine, ModelPermissions};
use distributed::{
    DistributedProjectManifest, RelationalReadModel, RelationshipDef, RelationshipKind,
};

use super::common::{
    engine_all_columns, error_messages, exec_json, seed_orders, session, ChildView, OrderView,
    ParentView,
};

/// A1: claim row filter on list (existing harden case).
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
    let data = exec_json(&engine, &a, "{ orders { order_id customer_id } }").await;
    let orders = data["orders"].as_array().unwrap();
    assert_eq!(orders.len(), 2);
    assert!(orders.iter().all(|o| o["customer_id"] == "tenant-a"));

    let b = session("user", "tenant-b");
    let data = exec_json(&engine, &b, "{ orders { order_id } }").await;
    assert_eq!(data["orders"].as_array().unwrap().len(), 1);

    // A2: by_pk cross-tenant → null
    let resp = engine
        .execute(
            &b,
            Request::new(r#"{ orders_by_pk(order_id: "o1") { order_id } }"#),
        )
        .await;
    assert!(resp.errors.is_empty(), "{:?}", resp.errors);
    let data = serde_json::to_value(&resp.data).unwrap();
    assert!(
        data["orders_by_pk"].is_null() || data.get("orders_by_pk").is_none(),
        "cross-tenant by_pk must not leak: {data}"
    );
}

/// A2 focused: claim filter on by_pk of own vs other tenant.
#[tokio::test]
async fn a2_claim_filter_on_by_pk_isolates_tenants() {
    let pool = seed_orders().await;
    let engine = GraphqlEngine::builder(pool)
        .roles(&["user"])
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
    let data = exec_json(
        &engine,
        &a,
        r#"{ orders_by_pk(order_id: "o1") { order_id customer_id } }"#,
    )
    .await;
    assert_eq!(data["orders_by_pk"]["order_id"], "o1");
    assert_eq!(data["orders_by_pk"]["customer_id"], "tenant-a");

    let b = session("user", "tenant-b");
    let data = exec_json(
        &engine,
        &b,
        r#"{ orders_by_pk(order_id: "o1") { order_id } }"#,
    )
    .await;
    assert!(
        data["orders_by_pk"].is_null(),
        "tenant-b must not read tenant-a by_pk: {data}"
    );
}

/// A3: claim filter on aggregate count.
#[tokio::test]
async fn a3_claim_filter_on_aggregate_count() {
    let pool = seed_orders().await;
    let engine = GraphqlEngine::builder(pool)
        .roles(&["user"])
        .model::<OrderView>(
            ModelPermissions::new().role(
                "user",
                select()
                    .all_columns()
                    .allow_aggregations(true)
                    .filter(col("customer_id").eq(claim("x-user-id"))),
            ),
        )
        .build()
        .unwrap();

    let a = session("user", "tenant-a");
    let data = exec_json(
        &engine,
        &a,
        "{ orders_aggregate { aggregate { count } } }",
    )
    .await;
    assert_eq!(
        data["orders_aggregate"]["aggregate"]["count"], 2,
        "tenant-a has 2 orders: {data}"
    );

    let b = session("user", "tenant-b");
    let data = exec_json(
        &engine,
        &b,
        "{ orders_aggregate { aggregate { count } } }",
    )
    .await;
    assert_eq!(
        data["orders_aggregate"]["aggregate"]["count"], 1,
        "tenant-b has 1 order: {data}"
    );
}

/// Column allowlist on root list (existing).
#[tokio::test]
async fn column_allowlist_denies_ungranted_fields() {
    let pool = seed_orders().await;
    let engine = GraphqlEngine::builder(pool)
        .roles(&["restricted", "user"])
        .model::<OrderView>(
            ModelPermissions::new()
                .role("restricted", select().columns(["order_id", "status"]))
                .role("user", select().all_columns()),
        )
        .build()
        .unwrap();

    let restricted = session("restricted", "tenant-a");
    let resp = engine
        .execute(
            &restricted,
            Request::new("{ orders { order_id status customer_id } }"),
        )
        .await;
    assert!(
        !resp.errors.is_empty(),
        "customer_id must be unknown for restricted role: {:?}",
        resp.errors
    );
    let msgs = error_messages(&resp);
    assert!(
        msgs.contains("customer_id") || msgs.contains("unknown field"),
        "expected unknown field for denied column, got {msgs}"
    );

    let data = exec_json(&engine, &restricted, "{ orders { order_id status } }").await;
    let row = &data["orders"][0];
    assert!(row.get("order_id").is_some());
    assert!(row.get("customer_id").is_none());
    assert!(row.get("total_cents").is_none());
}

/// A5: nested relationship cannot project ungranted child columns.
#[tokio::test]
async fn a5_nested_relationship_column_allowlist_denies() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
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
         INSERT INTO children VALUES ('c1', 'p1', 'secret-name');",
    )
    .execute(&pool)
    .await
    .unwrap();

    let mut parent = ParentView::schema().clone();
    parent.relationships = vec![RelationshipDef {
        field_name: "children".into(),
        kind: RelationshipKind::HasMany,
        target_model: "ChildView".into(),
        foreign_key: Some("parent_id".into()),
        through: None,
        target_foreign_key: None,
    }];
    let child = ChildView::schema().clone();
    let manifest = DistributedProjectManifest::new("rel-authz")
        .table_schema(parent)
        .table_schema(child);

    // Parent: all columns; child: only child_id (not name).
    let engine = GraphqlEngine::from_manifest(&manifest, pool)
        .unwrap()
        .roles(&["user"])
        .permission::<ParentView>("user", select().all_columns())
        .permission::<ChildView>("user", select().columns(["child_id", "parent_id"]))
        .build()
        .expect("build");

    let s = session("user", "u");
    let resp = engine
        .execute(
            &s,
            Request::new("{ parents { parent_id children { child_id name } } }"),
        )
        .await;
    assert!(
        !resp.errors.is_empty(),
        "child.name must be denied for restricted child grants: {:?}",
        resp.errors
    );
    let msgs = error_messages(&resp);
    assert!(
        msgs.contains("name") || msgs.contains("unknown field"),
        "expected unknown field for nested denied column, got {msgs}"
    );

    // Allowed nested selection still works.
    let data = exec_json(
        &engine,
        &s,
        "{ parents { parent_id children { child_id } } }",
    )
    .await;
    let children = data["parents"][0]["children"].as_array().unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0]["child_id"], "c1");
    assert!(children[0].get("name").is_none());
}

#[tokio::test]
async fn nested_has_many_relationship_e2e() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
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

    let mut parent = ParentView::schema().clone();
    parent.relationships = vec![RelationshipDef {
        field_name: "children".into(),
        kind: RelationshipKind::HasMany,
        target_model: "ChildView".into(),
        foreign_key: Some("parent_id".into()),
        through: None,
        target_foreign_key: None,
    }];
    let child = ChildView::schema().clone();
    let manifest = DistributedProjectManifest::new("rel")
        .table_schema(parent)
        .table_schema(child);

    let engine = GraphqlEngine::from_manifest(&manifest, pool)
        .unwrap()
        .roles(&["user"])
        .grant_all("user")
        .build()
        .expect("build");

    let s = session("user", "u");
    let data = exec_json(
        &engine,
        &s,
        "{ parents { parent_id children { child_id name } } }",
    )
    .await;
    let children = data["parents"][0]["children"].as_array().unwrap();
    assert_eq!(children.len(), 2);
}

#[tokio::test]
async fn json_looking_string_column_stays_string() {
    let pool = seed_orders().await;
    let engine = engine_all_columns(pool);
    let s = session("user", "tenant-a");
    let data = exec_json(&engine, &s, r#"{ orders_by_pk(order_id: "o1") { note } }"#).await;
    assert!(
        data["orders_by_pk"]["note"].is_string(),
        "note must remain string, got {:?}",
        data["orders_by_pk"]["note"]
    );
    assert_eq!(data["orders_by_pk"]["note"], "{\"looks\":\"json\"}");
}
