//! Same behavioral cases against an in-process **multi-service** topology
//! (catalog + orders handlers in separate Services, gateway proxies commands).
//!
//! Assertions are identical to `behavioral.rs` — only the process layout differs.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use distributed::bus::SqliteBus;
use distributed::graphql::graphql_router;
use distributed::microsvc::serve;
use distributed::{SqliteLockManager, SqliteRepository};
use serde_json::json;
use workshop_service::{
    build_catalog_service, build_full_service, build_graphql_engine, build_orders_service,
    distributed_manifest, identity_from_env,
};
use workshop_suite::{cases, graphql, post_command, wait_ready};

const BUS: &str = "workshop-multi-suite";

async fn boot_multi() -> String {
    // Shared file-less memory DB via shared pool: use one repo for all Services.
    let repo = SqliteRepository::connect_and_migrate("sqlite::memory:")
        .await
        .expect("repo");
    let registry = distributed_manifest().table_registry().expect("reg");
    repo.bootstrap_table_schema_for_dev(&registry)
        .await
        .expect("boot");
    let locks = SqliteLockManager::new(repo.pool().clone());
    let pool = repo.pool().clone();
    let bus_init = SqliteBus::new(pool.clone()).group(BUS);
    bus_init.ensure_tables().await.expect("bus tables");

    // Pick free ports
    let c = free_port().await;
    let o = free_port().await;
    let g = free_port().await;

    let catalog = Arc::new(
        build_catalog_service(repo.clone(), locks.clone(), repo.clone())
            .with_bus(SqliteBus::new(pool.clone()).group(BUS)),
    );
    let orders = Arc::new(
        build_orders_service(repo.clone(), locks.clone(), repo.clone())
            .with_bus(SqliteBus::new(pool.clone()).group(BUS)),
    );

    // Consumer for bus events (optional dual path)
    let cr = repo.clone();
    let cl = locks.clone();
    tokio::spawn(async move {
        loop {
            let bus = SqliteBus::new(cr.pool().clone()).group(BUS);
            let _ = build_full_service(cr.clone(), cl.clone(), cr.clone())
                .with_bus(bus)
                .run(distributed::bus::RunOptions::idempotent())
                .await;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });

    let c_bind = c.clone();
    let cat = Arc::clone(&catalog);
    tokio::spawn(async move {
        let _ = serve(cat, &c_bind).await;
    });
    let o_bind = o.clone();
    let ord = Arc::clone(&orders);
    tokio::spawn(async move {
        let _ = serve(ord, &o_bind).await;
    });

    let gql = Arc::new(build_graphql_engine(pool, identity_from_env()).expect("gql"));
    let catalog_url = format!("http://{c}");
    let orders_url = format!("http://{o}");
    let client = reqwest::Client::new();
    let app = Router::new()
        .merge(graphql_router(gql))
        .fallback(any(move |req: Request| {
            let client = client.clone();
            let catalog_url = catalog_url.clone();
            let orders_url = orders_url.clone();
            async move { proxy(client, catalog_url, orders_url, req).await }
        }));

    let addr: SocketAddr = g.parse().unwrap();
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let base = format!("http://{g}");
    assert!(
        wait_ready(&base, Duration::from_secs(10)).await,
        "multi gateway not ready"
    );
    base
}

async fn free_port() -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let a = l.local_addr().unwrap();
    drop(l);
    a.to_string()
}

async fn proxy(
    client: reqwest::Client,
    catalog_url: String,
    orders_url: String,
    req: Request,
) -> Response {
    let path = req.uri().path().trim_start_matches('/');
    let base = if path.starts_with("product.") {
        catalog_url
    } else if path.starts_with("workshop_order.") {
        orders_url
    } else {
        return (StatusCode::NOT_FOUND, path.to_string()).into_response();
    };
    let url = format!("{base}/{path}");
    let headers = req.headers().clone();
    let body = axum::body::to_bytes(req.into_body(), 1024 * 1024)
        .await
        .unwrap_or_default();
    let mut b = client.post(url).body(body.to_vec());
    for (k, v) in headers.iter() {
        if k == axum::http::header::HOST {
            continue;
        }
        if let Ok(v) = v.to_str() {
            b = b.header(k.as_str(), v);
        }
    }
    match b.send().await {
        Ok(r) => {
            let st = StatusCode::from_u16(r.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            (st, r.bytes().await.unwrap_or_default()).into_response()
        }
        Err(e) => (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    }
}

#[tokio::test]
async fn multi_same_cases_as_monolith() {
    let base = boot_multi().await;
    let pid = format!("p-{}", now());
    let oid = format!("o-{}", now());

    post_command(
        &base,
        "product.list",
        json!({
            "product_id": pid,
            "name": "Split mug",
            "price_cents": 900,
            "owner_id": "maker-1"
        }),
        "maker-1",
        "maker",
    )
    .await
    .expect(cases::LIST_PRODUCT);

    post_command(
        &base,
        "workshop_order.place",
        json!({
            "order_id": oid,
            "product_id": pid,
            "customer_id": "customer-1",
            "quantity": 1
        }),
        "customer-1",
        "customer",
    )
    .await
    .expect(cases::PLACE_ORDER);

    let products = graphql(
        &base,
        "{ products { product_id name } }",
        "admin-1",
        "admin",
    )
    .await
    .expect(cases::GRAPHQL_PRODUCTS);
    let arr = products["data"]["products"].as_array().expect("products");
    assert!(
        arr.iter().any(|r| r["product_id"] == pid),
        "{} missing product",
        cases::GRAPHQL_PRODUCTS
    );

    let orders = graphql(
        &base,
        "{ workshop_orders { order_id customer_id } }",
        "customer-1",
        "customer",
    )
    .await
    .expect(cases::GRAPHQL_ORDER_ISOLATION);
    let oarr = orders["data"]["workshop_orders"]
        .as_array()
        .expect("orders");
    assert!(
        oarr.iter().any(|r| r["order_id"] == oid),
        "{} missing order",
        cases::GRAPHQL_ORDER_ISOLATION
    );
    for r in oarr {
        assert_eq!(r["customer_id"], "customer-1");
    }
    eprintln!("multi_service: all case IDs green vs split topology");
}

fn now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
        .to_string()
}
