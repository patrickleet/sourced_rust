//! In-process multi-service topology for the shared suite.
//!
//! Two HTTP Services (catalog + orders) share SQLite + bus. Gateway binds
//! `GATEWAY_BIND`: GraphQL + proxies commands by name prefix to the right BC.
//!
//! Env: `DATABASE_URL`, `CATALOG_BIND`, `ORDERS_BIND`, `GATEWAY_BIND`

use std::env;
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
use workshop_runner_split::{
    open_repo, spawn_consumer, spawn_outbox_worker, ConsumerMode, BUS_GROUP,
};
use workshop_service::{
    build_catalog_service, build_graphql_engine, build_orders_service, identity_from_env,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let database_url = env::var("DATABASE_URL")
        .unwrap_or_else(|_| "sqlite:./workshop-split.db?mode=rwc".into());
    let catalog_bind = env::var("CATALOG_BIND").unwrap_or_else(|_| "127.0.0.1:8792".into());
    let orders_bind = env::var("ORDERS_BIND").unwrap_or_else(|_| "127.0.0.1:8793".into());
    let gateway_bind = env::var("GATEWAY_BIND").unwrap_or_else(|_| "127.0.0.1:8794".into());

    let (repo, locks) = open_repo(&database_url).await?;
    spawn_outbox_worker(repo.clone(), "workshop-split");
    spawn_consumer(repo.clone(), locks.clone(), ConsumerMode::Full);

    let pool = repo.pool().clone();
    let gql = Arc::new(build_graphql_engine(pool.clone(), identity_from_env())?);

    let catalog = Arc::new(
        build_catalog_service(repo.clone(), locks.clone(), repo.clone())
            .with_bus(SqliteBus::new(pool.clone()).group(BUS_GROUP)),
    );
    let orders = Arc::new(
        build_orders_service(repo.clone(), locks, repo)
            .with_bus(SqliteBus::new(pool).group(BUS_GROUP)),
    );

    let c = Arc::clone(&catalog);
    let c_bind = catalog_bind.clone();
    tokio::spawn(async move {
        if let Err(e) = serve(c, &c_bind).await {
            eprintln!("catalog serve: {e}");
        }
    });
    let o = Arc::clone(&orders);
    let o_bind = orders_bind.clone();
    tokio::spawn(async move {
        if let Err(e) = serve(o, &o_bind).await {
            eprintln!("orders serve: {e}");
        }
    });

    tokio::time::sleep(Duration::from_millis(200)).await;

    let catalog_url = format!("http://{catalog_bind}");
    let orders_url = format!("http://{orders_bind}");
    let client = reqwest::Client::new();
    let gql_router = graphql_router(gql);

    let app = Router::new().merge(gql_router).fallback(any(move |req: Request| {
        let client = client.clone();
        let catalog_url = catalog_url.clone();
        let orders_url = orders_url.clone();
        async move { proxy_command(client, catalog_url, orders_url, req).await }
    }));

    let addr: SocketAddr = gateway_bind.parse()?;
    eprintln!(
        "workshop-split gateway on http://{gateway_bind} (catalog={catalog_bind} orders={orders_bind})"
    );
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn proxy_command(
    client: reqwest::Client,
    catalog_url: String,
    orders_url: String,
    req: Request,
) -> Response {
    let path = req.uri().path().trim_start_matches('/');
    let target_base = if path.starts_with("product.") {
        catalog_url
    } else if path.starts_with("workshop_order.") {
        orders_url
    } else {
        return (StatusCode::NOT_FOUND, format!("unknown route {path}")).into_response();
    };
    let url = format!("{target_base}/{path}");
    let headers = req.headers().clone();
    let body = axum::body::to_bytes(req.into_body(), 1024 * 1024)
        .await
        .unwrap_or_default();

    let mut builder = client.post(&url).body(body.to_vec());
    for (k, v) in headers.iter() {
        if k == axum::http::header::HOST {
            continue;
        }
        if let Ok(v) = v.to_str() {
            builder = builder.header(k.as_str(), v);
        }
    }
    match builder.send().await {
        Ok(resp) => {
            let status =
                StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let bytes = resp.bytes().await.unwrap_or_default();
            (status, bytes).into_response()
        }
        Err(e) => (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    }
}
