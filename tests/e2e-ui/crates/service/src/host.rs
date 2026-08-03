//! One-screen host bootstrap for the e2e-ui application (task 13).
//!
//! The runner binary should only select environment and call [`run_e2e_host`].
//! Dialect adapters, outbox/consumer workers, GraphQL attachment, and Zitadel
//! extension scheduling live here as framework-facing process wiring.

use std::sync::Arc;
use std::time::Duration;

use distributed::bus::{PostgresBus, RunOptions, SqliteBus};
use distributed::command_dispatch::LocalCommandDispatcher;
use distributed::graphql::IdentityConfig;
use distributed::{
    BusPublisher, OutboxDispatcher, PostgresLockManager, PostgresRepository, SqliteLockManager,
    SqliteRepository,
};

use crate::{
    build_graphql_engine, build_service, distributed_manifest, serve_with_oidc, spawn_scrape_loop,
    ZitadelScrapeConfig, E2E_UI_APPLICATION,
};

const BUS_GROUP: &str = "e2e-ui";

/// Bind address and identity for one local process host.
pub struct HostOptions {
    pub bind: String,
    pub identity: IdentityConfig,
}

/// Start the e2e-ui full-local process for SQLite or Postgres from `DATABASE_URL`.
pub async fn run_e2e_host(
    database_url: &str,
    options: HostOptions,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    eprintln!(
        "e2e-ui host application=`{}` bind={}",
        E2E_UI_APPLICATION, options.bind
    );
    if database_url.starts_with("postgres://") || database_url.starts_with("postgresql://") {
        run_postgres(database_url, options).await
    } else {
        run_sqlite(database_url, options).await
    }
}

async fn run_sqlite(
    database_url: &str,
    options: HostOptions,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let repo = SqliteRepository::connect_and_migrate(database_url).await?;
    let registry = distributed_manifest()
        .table_registry()
        .map_err(|e| format!("manifest: {e}"))?;
    repo.bootstrap_table_schema_for_dev(&registry).await?;
    let locks = SqliteLockManager::new(repo.pool().clone());

    let bus = SqliteBus::new(repo.pool().clone()).group(BUS_GROUP);
    bus.ensure_tables().await?;

    let change_rx = repo.read_model_changes();
    let service = build_service(repo.clone(), locks.clone(), repo.clone())
        .with_bus(SqliteBus::new(repo.pool().clone()).group(BUS_GROUP));
    let gql = build_graphql_engine(&repo, &service, options.identity.clone(), Some(change_rx))?;
    let service = Arc::new(service.try_with_graphql(gql)?);
    // Public host boundary is the command dispatcher; HTTP OIDC serve still
    // consumes Service until microsvc GraphQL routes bind dispatcher directly.
    let _dispatcher = Arc::new(LocalCommandDispatcher::new(Arc::clone(&service)));

    spawn_outbox_sqlite(repo.clone());
    spawn_consumer_sqlite(repo.clone(), locks);
    spawn_zitadel_scrape(repo.clone());

    eprintln!("e2e-ui (sqlite) listening on http://{}", options.bind);
    serve_with_oidc(service, options.identity, &options.bind).await?;
    Ok(())
}

async fn run_postgres(
    database_url: &str,
    options: HostOptions,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let repo = PostgresRepository::connect_and_migrate(database_url).await?;
    let registry = distributed_manifest()
        .table_registry()
        .map_err(|e| format!("manifest: {e}"))?;
    repo.bootstrap_table_schema_for_dev(&registry).await?;
    let locks = PostgresLockManager::new(repo.pool().clone());

    let bus = PostgresBus::new(repo.pool().clone()).group(BUS_GROUP);
    bus.ensure_tables().await?;

    let change_rx = repo.read_model_changes();
    let service = build_service(repo.clone(), locks.clone(), repo.clone())
        .with_bus(PostgresBus::new(repo.pool().clone()).group(BUS_GROUP));
    let gql = build_graphql_engine(&repo, &service, options.identity.clone(), Some(change_rx))?;
    let service = Arc::new(service.try_with_graphql(gql)?);
    let _dispatcher = Arc::new(LocalCommandDispatcher::new(Arc::clone(&service)));

    spawn_outbox_postgres(repo.clone());
    spawn_consumer_postgres(repo.clone(), locks);
    spawn_zitadel_scrape(repo.clone());

    eprintln!("e2e-ui (postgres) listening on http://{}", options.bind);
    serve_with_oidc(service, options.identity, &options.bind).await?;
    Ok(())
}

fn spawn_zitadel_scrape<R>(repo: R)
where
    R: distributed::TransactionalCommit + Clone + Send + Sync + 'static,
{
    match ZitadelScrapeConfig::from_env() {
        Some(cfg) if cfg.background_enabled() || cfg.on_start => {
            eprintln!(
                "zitadel scrape: enabled (api={}, interval={}s, on_start={})",
                cfg.api_base,
                cfg.interval.as_secs(),
                cfg.on_start
            );
            spawn_scrape_loop(repo, cfg);
        }
        Some(_) => {
            eprintln!(
                "zitadel scrape: credentials present, background off (interval=0); use POST /zitadel.scrape.v1"
            );
        }
        None => {
            eprintln!(
                "zitadel scrape: disabled (set ZITADEL_SERVICE_USER_TOKEN + OIDC_ISSUER/ZITADEL_API_URL)"
            );
        }
    }
}

fn spawn_outbox_sqlite(repo: SqliteRepository) {
    tokio::spawn(async move {
        let bus = Arc::new(SqliteBus::new(repo.pool().clone()).group(BUS_GROUP));
        let dispatcher = OutboxDispatcher::new(
            repo.outbox_store(),
            BusPublisher::new(bus),
            format!("outbox:{}", std::process::id()),
            Duration::from_secs(30),
            5,
        )
        .with_service("e2e-ui");
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

fn spawn_consumer_sqlite(repo: SqliteRepository, locks: SqliteLockManager) {
    tokio::spawn(async move {
        loop {
            let bus = SqliteBus::new(repo.pool().clone()).group(BUS_GROUP);
            let service = build_service(repo.clone(), locks.clone(), repo.clone()).with_bus(bus);
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

fn spawn_outbox_postgres(repo: PostgresRepository) {
    tokio::spawn(async move {
        let bus = Arc::new(PostgresBus::new(repo.pool().clone()).group(BUS_GROUP));
        let dispatcher = OutboxDispatcher::new(
            repo.outbox_store(),
            BusPublisher::new(bus),
            format!("outbox:{}", std::process::id()),
            Duration::from_secs(30),
            5,
        )
        .with_service("e2e-ui");
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

fn spawn_consumer_postgres(repo: PostgresRepository, locks: PostgresLockManager) {
    tokio::spawn(async move {
        loop {
            let bus = PostgresBus::new(repo.pool().clone()).group(BUS_GROUP);
            let service = build_service(repo.clone(), locks.clone(), repo.clone()).with_bus(bus);
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
