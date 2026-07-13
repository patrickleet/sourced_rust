//! Composable route bundles — monolith registers all; microservices pick one.

use distributed::graphql::{
    select, GraphqlEngine, IdentityConfig, ModelPermissions, OidcConfig,
};
use distributed::microsvc::{
    ConfigurableOutboxPublisher, HasOutboxStore, HasRepo, Routes, Service,
};
use distributed::{AggregateBuilder, AggregateRepository, Queueable, QueuedRepository};
// Queueable::queued_with is used on EventStore leaves (e.g. SqliteRepository).
use sqlx::SqlitePool;
use workshop_catalog_domain::Product;
use workshop_orders_domain::WorkshopOrder;
use workshop_readmodels::{OrderView, ProductView};

use crate::bounds::{EventStore, Locks, ReadStore};
use crate::handlers;

/// Full monolith service (both BCs).
pub fn build_full_service<R, L, S>(repo: R, locks: L, read_models: S) -> Service
where
    R: EventStore,
    L: Locks,
    S: ReadStore,
    QueuedRepository<R, L>: Clone
        + AggregateBuilder
        + HasOutboxStore
        + distributed::TransactionalCommit
        + Send
        + Sync
        + 'static,
    AggregateRepository<QueuedRepository<R, L>, Product>:
        HasRepo + HasOutboxStore + ConfigurableOutboxPublisher + Send + Sync + 'static,
    AggregateRepository<QueuedRepository<R, L>, WorkshopOrder>:
        HasRepo + HasOutboxStore + ConfigurableOutboxPublisher + Send + Sync + 'static,
{
    let catalog = distributed::routes!(
        Routes::new()
            .with_repo(repo.clone().queued_with(locks.clone()).aggregate::<Product>())
            .with_read_model_store(read_models.clone()),
        command handlers::commands::list_product,
        event handlers::events::product_listed,
    );
    let orders = distributed::routes!(
        Routes::new()
            .with_repo(repo.queued_with(locks).aggregate::<WorkshopOrder>())
            .with_read_model_store(read_models),
        command handlers::commands::place_order,
        event handlers::events::workshop_order_placed,
    );
    Service::new()
        .named("workshop-monolith")
        .routes(catalog)
        .routes(orders)
}

/// Catalog microservice (handlers for Product only).
pub fn build_catalog_service<R, L, S>(repo: R, locks: L, read_models: S) -> Service
where
    R: EventStore,
    L: Locks,
    S: ReadStore,
    QueuedRepository<R, L>: Clone
        + AggregateBuilder
        + HasOutboxStore
        + distributed::TransactionalCommit
        + Send
        + Sync
        + 'static,
    AggregateRepository<QueuedRepository<R, L>, Product>:
        HasRepo + HasOutboxStore + ConfigurableOutboxPublisher + Send + Sync + 'static,
{
    let catalog = distributed::routes!(
        Routes::new()
            .with_repo(repo.queued_with(locks).aggregate::<Product>())
            .with_read_model_store(read_models),
        command handlers::commands::list_product,
        event handlers::events::product_listed,
    );
    Service::new().named("workshop-catalog").routes(catalog)
}

/// Orders microservice (handlers for WorkshopOrder only).
pub fn build_orders_service<R, L, S>(repo: R, locks: L, read_models: S) -> Service
where
    R: EventStore,
    L: Locks,
    S: ReadStore,
    QueuedRepository<R, L>: Clone
        + AggregateBuilder
        + HasOutboxStore
        + distributed::TransactionalCommit
        + Send
        + Sync
        + 'static,
    AggregateRepository<QueuedRepository<R, L>, WorkshopOrder>:
        HasRepo + HasOutboxStore + ConfigurableOutboxPublisher + Send + Sync + 'static,
{
    let orders = distributed::routes!(
        Routes::new()
            .with_repo(repo.queued_with(locks).aggregate::<WorkshopOrder>())
            .with_read_model_store(read_models),
        command handlers::commands::place_order,
        event handlers::events::workshop_order_placed,
    );
    Service::new().named("workshop-orders").routes(orders)
}

/// GraphQL engine over workshop read models.
pub fn build_graphql_engine(
    pool: SqlitePool,
    identity: IdentityConfig,
) -> Result<GraphqlEngine, String> {
    GraphqlEngine::builder(pool)
        .roles(&["customer", "admin", "maker", "user"])
        .model::<ProductView>(
            ModelPermissions::new()
                .role("customer", select().all_columns())
                .role("admin", select().all_columns())
                .role(
                    "maker",
                    select().all_columns().filter(
                        distributed::graphql::col("owner_id")
                            .eq(distributed::graphql::claim("x-user-id")),
                    ),
                )
                .role("user", select().all_columns()),
        )
        .model::<OrderView>(
            ModelPermissions::new()
                .role(
                    "customer",
                    select().all_columns().filter(
                        distributed::graphql::col("customer_id")
                            .eq(distributed::graphql::claim("x-user-id")),
                    ),
                )
                .role("admin", select().all_columns())
                .role("maker", select().all_columns())
                .role(
                    "user",
                    select().all_columns().filter(
                        distributed::graphql::col("customer_id")
                            .eq(distributed::graphql::claim("x-user-id")),
                    ),
                ),
        )
        .identity(identity)
        .graphiql(true)
        .build()
        .map_err(|e| e.to_string())
}

/// DevHeaders identity for local / suite (never production).
pub fn dev_identity() -> IdentityConfig {
    IdentityConfig::dev_headers()
}

/// OidcBearer from env when OIDC_ISSUER set; else DevHeaders for local DX.
pub fn identity_from_env() -> IdentityConfig {
    let iss = std::env::var("OIDC_ISSUER").unwrap_or_default();
    let aud = std::env::var("OIDC_AUDIENCE").unwrap_or_default();
    if iss.is_empty() || aud.is_empty() {
        return dev_identity();
    }
    let mut oidc = OidcConfig::new(iss, aud);
    if let Ok(jwks) = std::env::var("OIDC_JWKS_URI") {
        if !jwks.is_empty() {
            oidc.jwks_uri = Some(jwks);
        }
    }
    oidc.claim_map.engine_roles =
        vec!["admin".into(), "customer".into(), "maker".into(), "user".into()];
    oidc.claim_map.role_claims = vec![
        "groups".into(),
        "roles".into(),
        "realm_access.roles".into(),
        "urn:zitadel:iam:org:project:roles".into(),
    ];
    IdentityConfig::oidc_bearer(oidc)
}
