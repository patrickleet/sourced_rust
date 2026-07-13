//! Shared boot helpers for split runners (same store/bus, different handlers).

use std::sync::Arc;
use std::time::Duration;

use distributed::bus::{RunOptions, SqliteBus};
use distributed::microsvc::{serve, Service};
use distributed::{BusPublisher, OutboxDispatcher, SqliteLockManager, SqliteRepository};
use workshop_service::{
    build_catalog_service, build_full_service, build_orders_service, distributed_manifest,
};

pub use workshop_service::{build_graphql_engine, identity_from_env};

pub const BUS_GROUP: &str = "workshop-split";

pub async fn open_repo(
    database_url: &str,
) -> Result<(SqliteRepository, SqliteLockManager), Box<dyn std::error::Error + Send + Sync>> {
    let repo = SqliteRepository::connect_and_migrate(database_url).await?;
    let registry = distributed_manifest()
        .table_registry()
        .map_err(|e| format!("manifest: {e}"))?;
    repo.bootstrap_table_schema_for_dev(&registry).await?;
    let locks = SqliteLockManager::new(repo.pool().clone());
    let bus = SqliteBus::new(repo.pool().clone()).group(BUS_GROUP);
    bus.ensure_tables().await?;
    Ok((repo, locks))
}

pub fn spawn_outbox_worker(repo: SqliteRepository, service_name: &'static str) {
    tokio::spawn(async move {
        let bus = Arc::new(SqliteBus::new(repo.pool().clone()).group(BUS_GROUP));
        let dispatcher = OutboxDispatcher::new(
            repo.outbox_store(),
            BusPublisher::new(bus),
            format!("outbox:{service_name}:{}", std::process::id()),
            Duration::from_secs(30),
            5,
        )
        .with_service(service_name);
        loop {
            match dispatcher.dispatch_batch(32).await {
                Ok(o) if o.published > 0 || o.claimed > 0 => {}
                Ok(_) => tokio::time::sleep(Duration::from_millis(25)).await,
                Err(e) => {
                    eprintln!("outbox: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    });
}

pub fn spawn_consumer(
    repo: SqliteRepository,
    locks: SqliteLockManager,
    mode: ConsumerMode,
) {
    tokio::spawn(async move {
        loop {
            let bus = SqliteBus::new(repo.pool().clone()).group(BUS_GROUP);
            let service = match mode {
                ConsumerMode::Catalog => {
                    build_catalog_service(repo.clone(), locks.clone(), repo.clone()).with_bus(bus)
                }
                ConsumerMode::Orders => {
                    build_orders_service(repo.clone(), locks.clone(), repo.clone()).with_bus(bus)
                }
                ConsumerMode::Full => {
                    build_full_service(repo.clone(), locks.clone(), repo.clone()).with_bus(bus)
                }
            };
            match service.run(RunOptions::idempotent()).await {
                Ok(()) => tokio::time::sleep(Duration::from_millis(25)).await,
                Err(e) => {
                    eprintln!("consumer: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    });
}

#[derive(Clone, Copy)]
pub enum ConsumerMode {
    Catalog,
    Orders,
    Full,
}

pub async fn serve_service(
    service: Service,
    bind: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    eprintln!("listening on http://{bind}");
    serve(Arc::new(service), bind).await?;
    Ok(())
}

