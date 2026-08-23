//! Celld example GraphQL host. Not the e2e-ui one-process playground.
//!
//! Todo create/complete wait-dispatch to celld; Chat/Blob stay in-process.

use std::sync::Arc;
use std::time::Duration;

use distributed::bus::{PostgresBus, SqliteBus};
use distributed::command_dispatch::SharedCommandHost;
use distributed::graphql::IdentityConfig;
use distributed::microsvc::{spawn_outbox_publish_loop, spawn_service_consumer_loop};
use distributed::{PostgresLockManager, PostgresRepository, SqliteLockManager, SqliteRepository};
use e2e_celld_todo::CelldTodoCommandHost;

use crate::oidc_layer::serve_with_oidc_and_host;
use crate::{
    build_graphql_engine, build_service, distributed_manifest, spawn_scrape_loop,
    ZitadelScrapeConfig, E2E_UI_APPLICATION,
};

const BUS_GROUP: &str = "e2e-celld";

pub struct HostOptions {
    pub bind: String,
    pub identity: IdentityConfig,
    pub celld_url: String,
}

pub async fn run(
    database_url: &str,
    options: HostOptions,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let celld_url = options.celld_url.trim_end_matches('/').to_string();
    eprintln!(
        "e2e-celld graphql application=`{}` bind={} CELLD_URL={}",
        E2E_UI_APPLICATION, options.bind, celld_url
    );
    if database_url.starts_with("postgres://") || database_url.starts_with("postgresql://") {
        run_postgres(database_url, options, celld_url).await
    } else {
        run_sqlite(database_url, options, celld_url).await
    }
}

async fn run_sqlite(
    database_url: &str,
    options: HostOptions,
    celld_url: String,
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
    let host: SharedCommandHost =
        Arc::new(CelldTodoCommandHost::new(celld_url, Arc::clone(&service)));

    spawn_outbox_publish_loop(
        repo.outbox_store(),
        Arc::new(SqliteBus::new(repo.pool().clone()).group(BUS_GROUP)),
        "e2e-celld",
        Duration::from_secs(30),
        5,
    );
    {
        let repo = repo.clone();
        let locks = locks.clone();
        spawn_service_consumer_loop(move || {
            let bus = SqliteBus::new(repo.pool().clone()).group(BUS_GROUP);
            build_service(repo.clone(), locks.clone(), repo.clone()).with_bus(bus)
        });
    }
    spawn_zitadel_scrape(repo.clone());

    eprintln!(
        "e2e-celld (sqlite) listening on http://{} — Todo create/complete → celld",
        options.bind
    );
    serve_with_oidc_and_host(service, host, options.identity, &options.bind).await?;
    Ok(())
}

async fn run_postgres(
    database_url: &str,
    options: HostOptions,
    celld_url: String,
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
    let host: SharedCommandHost =
        Arc::new(CelldTodoCommandHost::new(celld_url, Arc::clone(&service)));

    spawn_outbox_publish_loop(
        repo.outbox_store(),
        Arc::new(PostgresBus::new(repo.pool().clone()).group(BUS_GROUP)),
        "e2e-celld",
        Duration::from_secs(30),
        5,
    );
    {
        let repo = repo.clone();
        let locks = locks.clone();
        spawn_service_consumer_loop(move || {
            let bus = PostgresBus::new(repo.pool().clone()).group(BUS_GROUP);
            build_service(repo.clone(), locks.clone(), repo.clone()).with_bus(bus)
        });
    }
    spawn_zitadel_scrape(repo.clone());

    eprintln!(
        "e2e-celld (postgres) listening on http://{} — Todo create/complete → celld",
        options.bind
    );
    serve_with_oidc_and_host(service, host, options.identity, &options.bind).await?;
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
