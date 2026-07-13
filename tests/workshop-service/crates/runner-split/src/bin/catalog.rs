//! Catalog microservice — product.list + product.listed projection.

use std::env;

use distributed::bus::SqliteBus;
use workshop_runner_split::{
    open_repo, serve_service, spawn_consumer, spawn_outbox_worker, ConsumerMode, BUS_GROUP,
};
use workshop_service::{build_catalog_service, build_graphql_engine, identity_from_env};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let database_url = env::var("DATABASE_URL")
        .unwrap_or_else(|_| "sqlite:./workshop-split.db?mode=rwc".into());
    let bind = env::var("BIND").unwrap_or_else(|_| "127.0.0.1:8792".into());
    let (repo, locks) = open_repo(&database_url).await?;
    spawn_outbox_worker(repo.clone(), "workshop-catalog");
    spawn_consumer(repo.clone(), locks.clone(), ConsumerMode::Catalog);

    let gql = build_graphql_engine(repo.pool().clone(), identity_from_env())?;
    let bus = SqliteBus::new(repo.pool().clone()).group(BUS_GROUP);
    let service = build_catalog_service(repo.clone(), locks, repo)
        .with_bus(bus)
        .with_graphql(gql);
    serve_service(service, &bind).await
}
