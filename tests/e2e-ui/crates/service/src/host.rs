//! One-screen e2e-ui host: compile plan, realize framework host, serve.
//!
//! Dialect connect, workers, dispatcher, and capability binding live in
//! [`RuntimeHost::realize_local`]. This file only names the application hooks
//! and ingress decoration.

use std::sync::Arc;

use axum::http::HeaderMap;
use axum::routing::post;
use axum::Json;
use distributed::application::{
    resolve_deployment, LocalSqlApplication, LocalSqlHandles, LocalSqlOptions, RuntimeHost,
};
use distributed::graphql::{GraphqlEngine, IdentityConfig};
use distributed::microsvc::Service;
use serde_json::Value;

use crate::application::{e2e_local_plan, E2E_UI_APPLICATION};
use crate::oidc_layer::{dispatch_named, serve_router_with_oidc};
use crate::{
    build_graphql_engine, build_service, distributed_manifest, spawn_scrape_loop,
    ZitadelScrapeConfig,
};

const BUS_GROUP: &str = "e2e-ui";

/// Bind address and identity for one local process host.
pub struct HostOptions {
    pub bind: String,
    pub identity: IdentityConfig,
}

#[derive(Clone)]
struct E2eUiApp;

impl LocalSqlApplication for E2eUiApp {
    fn compose(&self, handles: &LocalSqlHandles) -> Service {
        match handles {
            LocalSqlHandles::Sqlite { repo, locks, bus } => {
                build_service(repo.clone(), locks.clone(), repo.clone()).with_bus(bus.clone())
            }
            LocalSqlHandles::Postgres { repo, locks, bus } => {
                build_service(repo.clone(), locks.clone(), repo.clone()).with_bus(bus.clone())
            }
        }
    }

    fn engine(
        &self,
        handles: &LocalSqlHandles,
        service: &Service,
        identity: IdentityConfig,
        change_rx: Option<tokio::sync::broadcast::Receiver<distributed::ReadModelChange>>,
    ) -> Result<GraphqlEngine, String> {
        match handles {
            LocalSqlHandles::Sqlite { repo, .. } => {
                build_graphql_engine(repo, service, identity, change_rx)
            }
            LocalSqlHandles::Postgres { repo, .. } => {
                build_graphql_engine(repo, service, identity, change_rx)
            }
        }
    }

    fn after_bound(&self, handles: &LocalSqlHandles) {
        match handles {
            LocalSqlHandles::Sqlite { repo, .. } => spawn_zitadel_scrape(repo.clone()),
            LocalSqlHandles::Postgres { repo, .. } => spawn_zitadel_scrape(repo.clone()),
        }
    }
}

/// Start the e2e-ui full-local process from `DATABASE_URL`.
pub async fn run_e2e_host(
    database_url: &str,
    options: HostOptions,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (manifest, plan) = e2e_local_plan();
    let resolved = resolve_deployment(&manifest, &plan)?;
    eprintln!(
        "e2e-ui host application=`{}` plan=`{}` process=`full` bind={}",
        E2E_UI_APPLICATION, resolved.plan, options.bind
    );
    let registry = distributed_manifest()
        .table_registry()
        .map_err(|error| format!("manifest: {error}"))?;
    let realized = RuntimeHost::realize_local(
        &plan,
        "full",
        LocalSqlOptions {
            database_url: database_url.to_string(),
            identity: options.identity.clone(),
            bus_group: BUS_GROUP.into(),
            registry,
        },
        E2eUiApp,
    )
    .await?;
    eprintln!(
        "e2e-ui host process=`{}` listening on http://{}",
        realized.host.process_id(),
        options.bind
    );
    let service = Arc::clone(&realized.service);
    let ingress = Arc::clone(&service);
    let scrape = Arc::clone(&service);
    let router = realized
        .graphql_router()
        .route(
            "/zitadel.ingress.v1",
            post(move |headers: HeaderMap, Json(input): Json<Value>| {
                let svc = ingress.clone();
                async move { dispatch_named(svc, headers, input, "zitadel.ingress.v1").await }
            }),
        )
        .route(
            "/zitadel.scrape.v1",
            post(move |headers: HeaderMap, Json(input): Json<Value>| {
                let svc = scrape.clone();
                async move { dispatch_named(svc, headers, input, "zitadel.scrape.v1").await }
            }),
        );
    serve_router_with_oidc(router, options.identity, &options.bind).await?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use distributed::application::Capability;

    #[tokio::test]
    async fn e2e_plan_realizes_through_framework_host() {
        let (manifest, plan) = e2e_local_plan();
        resolve_deployment(&manifest, &plan).unwrap();
        let registry = distributed_manifest()
            .table_registry()
            .expect("e2e table registry");
        let realized = RuntimeHost::realize_local(
            &plan,
            "full",
            LocalSqlOptions {
                database_url: "sqlite::memory:".into(),
                identity: crate::dev_identity(),
                bus_group: "e2e-test".into(),
                registry,
            },
            E2eUiApp,
        )
        .await
        .expect("realize e2e plan");
        assert_eq!(realized.host.process_id(), "full");
        assert!(realized.host.dispatcher().is_some());
        assert!(realized.host.providers.contains(Capability::EventStore));
        assert!(realized
            .host
            .providers
            .contains(Capability::LocalCommandDispatch));
        assert!(!realized
            .host
            .providers
            .contains(Capability::RemoteCommandDispatch));
    }
}
