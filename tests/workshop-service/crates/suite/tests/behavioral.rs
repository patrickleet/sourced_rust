//! Shared behavioral suite — same cases for monolith and multi-service.
//!
//! Set `WORKSHOP_BASE_URL` to the process under test, or leave unset to boot
//! an **in-process monolith** (SQLite memory + InMemoryBus).
//!
//! Multi-service: start `workshop-split-all` and set
//! `WORKSHOP_BASE_URL=http://127.0.0.1:8794`.

use std::sync::Arc;
use std::time::Duration;

use distributed::bus::{InMemoryBus, RunOptions};
use distributed::microsvc::serve;
use distributed::{SqliteLockManager, SqliteRepository};
use serde_json::json;
use workshop_service::{
    build_full_service, build_graphql_engine, distributed_manifest, identity_from_env,
};
use workshop_suite::{cases, graphql, post_command, wait_ready};

/// Boot monolith in-process when WORKSHOP_BASE_URL is unset.
async fn ensure_target() -> String {
    if let Ok(url) = std::env::var("WORKSHOP_BASE_URL") {
        if !url.is_empty() {
            assert!(
                wait_ready(&url, Duration::from_secs(30)).await,
                "WORKSHOP_BASE_URL={url} not ready"
            );
            return url;
        }
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let bind = addr.to_string();
    let base = format!("http://{bind}");

    let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite::memory:".into());
    let repo = SqliteRepository::connect_and_migrate(&database_url)
        .await
        .expect("repo");
    let registry = distributed_manifest().table_registry().expect("registry");
    repo.bootstrap_table_schema_for_dev(&registry)
        .await
        .expect("bootstrap");
    let locks = SqliteLockManager::new(repo.pool().clone());
    let bus = InMemoryBus::new();

    let gql = build_graphql_engine(repo.pool().clone(), identity_from_env()).expect("gql");
    let service = Arc::new(
        build_full_service(repo.clone(), locks.clone(), repo.clone())
            .with_bus(bus.clone())
            .with_graphql(gql),
    );

    // Consumer: process product.listed / workshop_order.placed projections.
    let consumer_repo = repo.clone();
    let consumer_locks = locks.clone();
    let bus_c = bus.clone();
    tokio::spawn(async move {
        loop {
            let service = build_full_service(
                consumer_repo.clone(),
                consumer_locks.clone(),
                consumer_repo.clone(),
            )
            .with_bus(bus_c.clone());
            let _ = service.run(RunOptions::idempotent()).await;
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    });

    let svc = Arc::clone(&service);
    let bind_c = bind.clone();
    tokio::spawn(async move {
        let _ = serve(svc, &bind_c).await;
    });

    assert!(
        wait_ready(&base, Duration::from_secs(10)).await,
        "in-process monolith not ready at {base}"
    );
    base
}

async fn poll_graphql_products(base: &str, product_id: &str) -> bool {
    let mut last = String::new();
    for _ in 0..80 {
        match graphql(
            base,
            "{ products { product_id name price_cents listed owner_id } }",
            "admin-1",
            "admin",
        )
        .await
        {
            Ok(v) => {
                last = v.to_string();
                if let Some(arr) = v["data"]["products"].as_array() {
                    if arr.iter().any(|r| r["product_id"] == product_id) {
                        return true;
                    }
                }
            }
            Err(e) => last = e,
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    eprintln!("poll_graphql_products last={last}");
    false
}

#[tokio::test]
async fn w1_list_product() {
    let base = ensure_target().await;
    let id = format!("p-{}", uuid_lite());
    let resp = post_command(
        &base,
        "product.list",
        json!({
            "product_id": id,
            "name": "Ceramic mug",
            "price_cents": 1800,
            "owner_id": "maker-1"
        }),
        "maker-1",
        "maker",
    )
    .await
    .unwrap_or_else(|e| panic!("{} failed: {e}", cases::LIST_PRODUCT));
    assert_eq!(resp["product_id"], id, "{}", cases::LIST_PRODUCT);
    eprintln!("{} ok {id}", cases::LIST_PRODUCT);
}

#[tokio::test]
async fn w2_place_order_and_graphql() {
    let base = ensure_target().await;
    let pid = format!("p-{}", uuid_lite());
    let oid = format!("o-{}", uuid_lite());

    post_command(
        &base,
        "product.list",
        json!({
            "product_id": pid,
            "name": "Bowl",
            "price_cents": 2200,
            "owner_id": "maker-1"
        }),
        "maker-1",
        "maker",
    )
    .await
    .expect(cases::LIST_PRODUCT);

    assert!(
        poll_graphql_products(&base, &pid).await,
        "{} product not projected",
        cases::GRAPHQL_PRODUCTS
    );
    eprintln!("{} ok product {pid}", cases::GRAPHQL_PRODUCTS);

    post_command(
        &base,
        "workshop_order.place",
        json!({
            "order_id": oid,
            "product_id": pid,
            "customer_id": "customer-1",
            "quantity": 2
        }),
        "customer-1",
        "customer",
    )
    .await
    .expect(cases::PLACE_ORDER);
    eprintln!("{} ok order {oid}", cases::PLACE_ORDER);

    let mut found = false;
    for _ in 0..80 {
        if let Ok(v) = graphql(
            &base,
            "{ workshop_orders { order_id product_id customer_id status } }",
            "customer-1",
            "customer",
        )
        .await
        {
            if let Some(arr) = v["data"]["workshop_orders"].as_array() {
                if arr.iter().any(|r| r["order_id"] == oid) {
                    found = true;
                    for r in arr {
                        assert_eq!(
                            r["customer_id"], "customer-1",
                            "{} isolation",
                            cases::GRAPHQL_ORDER_ISOLATION
                        );
                    }
                    break;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        found,
        "{} order not in GraphQL",
        cases::GRAPHQL_ORDER_ISOLATION
    );
    eprintln!("{} ok", cases::GRAPHQL_ORDER_ISOLATION);
}

fn uuid_lite() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{n:x}")
}
