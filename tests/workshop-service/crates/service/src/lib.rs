//! Workshop service library — handlers + route bundles.
//!
//! **Monolith:** register both `catalog_routes` and `orders_routes` on one `Service`.
//! **Microservices:** run two Services, each with one route bundle, sharing bus + store.
//! Domain and readmodel crates stay identical; only **where handlers run** changes.

mod bounds;
mod deps;
pub mod handlers;
mod service;

pub use service::{
    build_catalog_service, build_full_service, build_graphql_engine, build_orders_service,
    dev_identity, identity_from_env,
};
pub use workshop_readmodels::distributed_manifest;
