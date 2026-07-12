//! Shared fixtures for GraphQL harden / red-team suites.
//! Re-exports `tests/support/graphql.rs` so existing modules keep `super::common::*`.

#[path = "../support/graphql.rs"]
mod graphql_support;

pub use graphql_support::*;
