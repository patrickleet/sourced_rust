//! Celld example GraphQL host. Not the e2e-ui one-process playground.
//!
//! Todo create/complete and chat.post wait-dispatch to celld. GraphQL `@live`
//! and Eventual projectors stay in this process.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use distributed::bus::NatsBus;
use distributed::command_dispatch::{CommandHost, HttpCommandHost, SharedCommandHost};
use distributed::bus::MessagePublisher;
use distributed::BusPublisher;
use distributed::graphql::protocol::ProtocolResponseAccumulator;
use distributed::graphql::{IdentityConfig, VerifiedPrincipal};
use distributed::microsvc::{
    spawn_outbox_publish_loop, spawn_service_consumer_loop, CausalCommandPublicStatus,
    CausalDispatchError, CausalDispatchResult, Service, Session,
};
use distributed::{PostgresLockManager, PostgresRepository, SqliteLockManager, SqliteRepository};
use e2e_celld_chat::CelldChatCommandHost;
use e2e_celld_todo::CelldTodoCommandHost;
use serde_json::Value;

use crate::oidc_layer::{serve_with_oidc_and_host, InternalOutboxDrain};
use crate::{
    build_graphql_engine, build_service, distributed_manifest, spawn_scrape_loop,
    ZitadelScrapeConfig, E2E_UI_APPLICATION,
};

const BUS_GROUP: &str = "e2e-celld";

pub struct HostOptions {
    pub bind: String,
    pub identity: IdentityConfig,
    pub celld_url: String,
    pub nats_url: String,
}

pub async fn run(
    database_url: &str,
    options: HostOptions,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let celld_url = options.celld_url.trim_end_matches('/').to_string();
    eprintln!(
        "e2e-celld graphql application=`{}` bind={} CELLD_URL={} NATS_URL={}",
        E2E_UI_APPLICATION, options.bind, celld_url, options.nats_url
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
    let nats = connect_nats(&options.nats_url).await?;

    let change_rx = repo.read_model_changes();
    let service = build_service(repo.clone(), locks.clone(), repo.clone()).with_bus(nats.clone());
    let gql = build_graphql_engine(&repo, &service, options.identity.clone(), Some(change_rx))?;
    let service = Arc::new(service.try_with_graphql(gql)?);
    let host: SharedCommandHost = Arc::new(CelldAppHost::new(
        celld_url.clone(),
        Arc::clone(&service),
        BusPublisher::new(Arc::new(nats.clone())),
    ));

    spawn_outbox_publish_loop(
        repo.outbox_store(),
        Arc::new(nats.clone()),
        "e2e-celld",
        Duration::from_secs(30),
        5,
    );
    {
        let repo = repo.clone();
        let locks = locks.clone();
        let nats = nats.clone();
        spawn_service_consumer_loop(move || {
            build_service(repo.clone(), locks.clone(), repo.clone()).with_bus(nats.clone())
        });
    }
    spawn_zitadel_scrape(repo.clone());

    eprintln!(
        "e2e-celld (sqlite) listening on http://{} — cell wait-path; bus drain; @live stays here",
        options.bind
    );
    serve_with_oidc_and_host(
        service,
        host,
        options.identity,
        &options.bind,
        Some(cell_alarm_drain(
            BusPublisher::new(Arc::new(nats)),
            celld_url,
        )),
    )
    .await?;
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
    let nats = connect_nats(&options.nats_url).await?;

    let change_rx = repo.read_model_changes();
    let service = build_service(repo.clone(), locks.clone(), repo.clone()).with_bus(nats.clone());
    let gql = build_graphql_engine(&repo, &service, options.identity.clone(), Some(change_rx))?;
    let service = Arc::new(service.try_with_graphql(gql)?);
    let host: SharedCommandHost = Arc::new(CelldAppHost::new(
        celld_url.clone(),
        Arc::clone(&service),
        BusPublisher::new(Arc::new(nats.clone())),
    ));

    spawn_outbox_publish_loop(
        repo.outbox_store(),
        Arc::new(nats.clone()),
        "e2e-celld",
        Duration::from_secs(30),
        5,
    );
    {
        let repo = repo.clone();
        let locks = locks.clone();
        let nats = nats.clone();
        spawn_service_consumer_loop(move || {
            build_service(repo.clone(), locks.clone(), repo.clone()).with_bus(nats.clone())
        });
    }
    spawn_zitadel_scrape(repo.clone());

    eprintln!(
        "e2e-celld (postgres) listening on http://{} — cell wait-path; bus drain; @live stays here",
        options.bind
    );
    serve_with_oidc_and_host(
        service,
        host,
        options.identity,
        &options.bind,
        Some(cell_alarm_drain(
            BusPublisher::new(Arc::new(nats)),
            celld_url,
        )),
    )
    .await?;
    Ok(())
}

/// Routes Todo and Chat wait-paths to their cells. Blob and identity stay local.
/// Cell outbox publishes through [`MessagePublisher`] (NATS in this example;
/// Kafka/Rabbit swap the bus constructor). Projectors stay in this process.
pub struct CelldAppHost<P> {
    todo: CelldTodoCommandHost<P>,
    chat: CelldChatCommandHost<P>,
}

impl<P> CelldAppHost<P>
where
    P: MessagePublisher + Clone + Send + Sync + 'static,
{
    pub fn new(celld_url: impl Into<String>, service: Arc<Service>, publisher: P) -> Self {
        let celld_url = celld_url.into();
        Self {
            todo: CelldTodoCommandHost::new(
                celld_url.clone(),
                Arc::clone(&service),
                publisher.clone(),
            ),
            chat: CelldChatCommandHost::new(celld_url, service, publisher),
        }
    }
}

fn cell_alarm_drain<P>(publisher: P, celld_url: String) -> InternalOutboxDrain
where
    P: MessagePublisher + Clone + Send + Sync + 'static,
{
    let http = HttpCommandHost::new(&celld_url);
    Arc::new(move |body: Value| {
        let http = http.clone();
        let publisher = publisher.clone();
        let celld_url = celld_url.clone();
        Box::pin(async move {
            let Some(kind) = body
                .get("kind")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                return;
            };
            let Some(id) = body
                .get("id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                return;
            };
            let rows = CausalDispatchResult::outbox_from_wait_path(&body);
            let shard = http.retarget(format!("{celld_url}/{kind}/{id}"));
            shard.drain_cell_outbox(&publisher, &rows).await;
        })
    })
}

async fn connect_nats(
    url: &str,
) -> Result<NatsBus, Box<dyn std::error::Error + Send + Sync>> {
    let bus = NatsBus::connect(url)
        .namespace("e2e-celld")
        .group(BUS_GROUP)
        .await?;
    bus.ensure_stream().await?;
    eprintln!("e2e-celld bus ready (nats {url}); swap connect_nats for Kafka/Rabbit");
    Ok(bus)
}

#[async_trait]
impl<P> CommandHost for CelldAppHost<P>
where
    P: MessagePublisher + Clone + Send + Sync + 'static,
{
    async fn invoke(
        &self,
        command: &str,
        command_id: &str,
        input: Value,
        session: Session,
        principal: VerifiedPrincipal,
        protocol: Option<ProtocolResponseAccumulator>,
    ) -> Result<CausalDispatchResult, CausalDispatchError> {
        if command == "chat.post" {
            return self
                .chat
                .invoke(command, command_id, input, session, principal, protocol)
                .await;
        }
        self.todo
            .invoke(command, command_id, input, session, principal, protocol)
            .await
    }

    async fn status(
        &self,
        command_id: &str,
        session: &Session,
        principal: VerifiedPrincipal,
        protocol: Option<ProtocolResponseAccumulator>,
    ) -> Result<CausalCommandPublicStatus, CausalDispatchError> {
        let chat = self
            .chat
            .status(command_id, session, principal.clone(), protocol.clone())
            .await?;
        if !chat.is_unknown() {
            return Ok(chat);
        }
        self.todo
            .status(command_id, session, principal, protocol)
            .await
    }
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
