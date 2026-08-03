//! e2e-ui service library — application modules + host.
//!
//! # Authoring story
//!
//! - [`application`] — surface names and module inventory
//! - [`modules`] — todo / chat / blob mounts + compose + GraphQL
//! - [`host`] — one-screen process bootstrap (`run_e2e_host`)
//! - [`handlers`] — command/event bodies (domain-adjacent, not infrastructure)
//!
//! Domain crates stay pure aggregates; read models live in `e2e-readmodels`;
//! portable projections in `e2e-projections`.

mod application;
mod bounds;
mod deps;
pub mod handlers;
mod host;
pub mod modules;
mod oidc_layer;
mod service;

pub use application::{
    DISTRIBUTED_ADMIN_CLIENT_SURFACE, DISTRIBUTED_CLIENT_SURFACE, DISTRIBUTED_PUBLIC_CLIENT_SURFACE,
    E2E_UI_APPLICATION, E2E_UI_MODULE_IDS,
};
pub use e2e_readmodels::distributed_manifest;
pub use handlers::ingestors::zitadel::{
    scrape_users_to_outbox, spawn_scrape_loop, ScrapeReport, ZitadelScrapeConfig,
};
pub use host::{run_e2e_host, HostOptions};
pub use oidc_layer::serve_with_oidc;
pub use service::{
    build_graphql_engine, build_service, dev_identity, distributed_admin_client_surface,
    distributed_client_surface, distributed_public_client_surface, identity_from_env,
    oidc_bearer_config,
};
