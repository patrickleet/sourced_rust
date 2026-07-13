//! Todo service runner.
//!
//! Env:
//! - `BIND` (default `127.0.0.1:8791`)
//! - `DATABASE_URL` (default `sqlite:./e2e-ui.db?mode=rwc`)
//! - `OIDC_ISSUER` / `OIDC_AUDIENCE` optional (else DevHeaders)

use std::env;
use std::sync::Arc;
use std::time::Duration;

use distributed::bus::{RunOptions, SqliteBus};
use distributed::microsvc::serve;
use distributed::{BusPublisher, OutboxDispatcher, SqliteLockManager, SqliteRepository};
use e2e_service::{
    build_graphql_engine, build_service, distributed_manifest, identity_from_env,
};

const BUS_GROUP: &str = "e2e-ui";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let database_url =
        env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite:./e2e-ui.db?mode=rwc".into());
    let bind = env::var("BIND").unwrap_or_else(|_| "127.0.0.1:8791".into());

    let repo = SqliteRepository::connect_and_migrate(&database_url).await?;
    let registry = distributed_manifest()
        .table_registry()
        .map_err(|e| format!("manifest: {e}"))?;
    repo.bootstrap_table_schema_for_dev(&registry).await?;
    let locks = SqliteLockManager::new(repo.pool().clone());

    let bus = SqliteBus::new(repo.pool().clone()).group(BUS_GROUP);
    bus.ensure_tables().await?;

    // Wire commit-path invalidation so GraphQL subscriptions push after projectors.
    let change_rx = repo.read_model_changes();
    let gql = build_graphql_engine(
        repo.pool().clone(),
        identity_from_env(),
        Some(change_rx),
    )?;
    let http_service = Arc::new(
        build_service(repo.clone(), locks.clone(), repo.clone())
            .with_bus(SqliteBus::new(repo.pool().clone()).group(BUS_GROUP))
            .with_graphql(gql),
    );

    let outbox_repo = repo.clone();
    tokio::spawn(async move {
        let bus = Arc::new(SqliteBus::new(outbox_repo.pool().clone()).group(BUS_GROUP));
        let dispatcher = OutboxDispatcher::new(
            outbox_repo.outbox_store(),
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

    let consumer_repo = repo.clone();
    let consumer_locks = locks.clone();
    tokio::spawn(async move {
        loop {
            let bus = SqliteBus::new(consumer_repo.pool().clone()).group(BUS_GROUP);
            let service = build_service(
                consumer_repo.clone(),
                consumer_locks.clone(),
                consumer_repo.clone(),
            )
            .with_bus(bus);
            match service.run(RunOptions::idempotent()).await {
                Ok(()) => tokio::time::sleep(Duration::from_millis(25)).await,
                Err(e) => {
                    eprintln!("consumer: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    });

    eprintln!("e2e-ui listening on http://{bind}");
    serve(http_service, &bind).await?;
    Ok(())
}
