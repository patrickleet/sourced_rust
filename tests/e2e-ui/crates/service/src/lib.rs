//! e2e-ui service library — handlers + GraphQL.
//!
//! Domain logic lives in `*-domain`, query models and read RBAC in
//! `e2e-readmodels`, and portable event mappings in `e2e-projections`. This
//! crate owns the explicit command/projector handlers, placement, GraphQL, and
//! Zitadel ingress. Eventual read models are written by projector handlers;
//! eligible direct projections are written in the command transaction.

mod application;
mod bounds;
mod deps;
pub mod handlers;
mod host;
mod oidc_layer;
mod service;

pub use application::{
    DISTRIBUTED_ADMIN_CLIENT_SURFACE, DISTRIBUTED_CLIENT_SURFACE, DISTRIBUTED_PUBLIC_CLIENT_SURFACE,
    E2E_UI_APPLICATION, E2E_UI_MODULE_IDS,
};
pub use host::{run_e2e_host, HostOptions};
pub use e2e_readmodels::distributed_manifest;
/// Zitadel Management API scrape (reconcile missed Action events).
pub use handlers::ingestors::zitadel::{
    scrape_users_to_outbox, spawn_scrape_loop, ScrapeReport, ZitadelScrapeConfig,
};
pub use oidc_layer::serve_with_oidc;
pub use service::{
    build_graphql_engine, build_service, dev_identity, distributed_admin_client_surface,
    distributed_client_surface, distributed_public_client_surface, identity_from_env,
    oidc_bearer_config,
};
