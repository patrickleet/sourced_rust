//! Todo service library — handlers + GraphQL.
//!
//! Domain logic lives in `todo-domain`. Read models live in `e2e-readmodels`.
//! This crate wires thin command handlers + event projectors.
//! **Commands never write read models** — only projectors do.

mod bounds;
mod deps;
pub mod handlers;
mod service;

pub use service::{
    build_graphql_engine, build_service, dev_identity, identity_from_env, oidc_bearer_config,
};
pub use e2e_readmodels::distributed_manifest;
