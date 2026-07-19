//! e2e-ui runner — SQLite (offline) or Postgres (compose stack).
//!
//! Env:
//! - `DATABASE_URL` — `sqlite:…` (default) or `postgres://…`
//! - `BIND` (default `127.0.0.1:8791`)
//! - `OIDC_ISSUER` / `OIDC_AUDIENCE` / `OIDC_JWKS_URI` → OidcBearer; else DevHeaders
//! - `ZITADEL_SERVICE_USER_TOKEN` + `OIDC_ISSUER`/`ZITADEL_API_URL` → periodic user scrape
//! - `ZITADEL_SCRAPE_INTERVAL_SECS` (default 60; `0` = no background loop)

use std::env;
use std::sync::Arc;
use std::time::Duration;

use distributed::bus::{PostgresBus, RunOptions, SqliteBus};
use distributed::{
    BusPublisher, OutboxDispatcher, PostgresLockManager, PostgresRepository, SqliteLockManager,
    SqliteRepository,
};
use e2e_service::{
    build_graphql_engine, build_service, distributed_manifest, identity_from_env, serve_with_oidc,
    spawn_scrape_loop, ZitadelScrapeConfig,
};

const BUS_GROUP: &str = "e2e-ui";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let database_url =
        env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite:./e2e-ui.db?mode=rwc".into());
    let bind = env::var("BIND").unwrap_or_else(|_| "127.0.0.1:8791".into());
    let identity = identity_from_env();

    if database_url.starts_with("postgres://") || database_url.starts_with("postgresql://") {
        run_postgres(&database_url, &bind, identity).await
    } else {
        run_sqlite(&database_url, &bind, identity).await
    }
}

async fn run_sqlite(
    database_url: &str,
    bind: &str,
    identity: distributed::graphql::IdentityConfig,
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
    let gql = build_graphql_engine(repo.pool().clone(), identity.clone(), Some(change_rx))?;
    let http_service = Arc::new(
        build_service(repo.clone(), locks.clone(), repo.clone())
            .with_bus(SqliteBus::new(repo.pool().clone()).group(BUS_GROUP))
            .with_graphql(gql),
    );

    spawn_outbox_sqlite(repo.clone());
    spawn_consumer_sqlite(repo.clone(), locks);
    spawn_zitadel_scrape(repo.clone());

    eprintln!("e2e-ui (sqlite) listening on http://{bind}");
    serve_with_oidc(http_service, identity, bind).await?;
    Ok(())
}

async fn run_postgres(
    database_url: &str,
    bind: &str,
    identity: distributed::graphql::IdentityConfig,
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
    let gql = build_graphql_engine(repo.pool().clone(), identity.clone(), Some(change_rx))?;
    let http_service = Arc::new(
        build_service(repo.clone(), locks.clone(), repo.clone())
            .with_bus(PostgresBus::new(repo.pool().clone()).group(BUS_GROUP))
            .with_graphql(gql),
    );

    spawn_outbox_postgres(repo.clone());
    spawn_consumer_postgres(repo.clone(), locks);
    spawn_zitadel_scrape(repo.clone());

    eprintln!("e2e-ui (postgres) listening on http://{bind}");
    serve_with_oidc(http_service, identity, bind).await?;
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
            eprintln!("zitadel scrape: credentials present, background off (interval=0); use POST /zitadel.scrape.v1");
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
