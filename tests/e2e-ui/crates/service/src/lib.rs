//! e2e-ui service library — handlers + GraphQL.
//!
//! Domain logic lives in `*-domain`. Read models live in `e2e-readmodels`.
//! This crate wires thin command handlers + event projectors + Zitadel ingress.
//! **Commands never write read models** — only projectors do (including auth_users
//! from provider messages published by the Zitadel ingestor).

mod bounds;
mod deps;
pub mod handlers;
mod oidc_layer;
mod service;

pub use oidc_layer::serve_with_oidc;
pub use service::{
    build_graphql_engine, build_service, dev_identity, graphql_commands, identity_from_env,
    oidc_bearer_config,
};
pub use e2e_readmodels::distributed_manifest;
/// Zitadel Management API scrape (reconcile missed Action events).
pub use handlers::ingestors::zitadel::{
    scrape_users_to_outbox, spawn_scrape_loop, ZitadelScrapeConfig, ScrapeReport,
};
