//! Auto-generated read-only GraphQL over relational read models.
//!
//! `naming` and `sdl` always compile (zero deps beyond the rest of the crate)
//! so `dctl schema --format graphql` works without enabling the `graphql`
//! feature. Execution, the dynamic schema, and the axum router sit behind
//! `feature = "graphql"`.

pub mod naming;
pub mod sdl;

pub use naming::{
    aggregate_field, by_pk_field, comparison_op_fields, include_postgres_json_comparison_ops,
    is_valid_graphql_name, object_type_name, root_list_field, scalar_type_name,
    POSTGRES_JSON_COMPARISON_OPS, PORTABLE_COMPARISON_OPS, STRING_COMPARISON_OPS,
};
pub use sdl::{graphql_sdl_for_tables, graphql_sdl_for_tables_with_options, SdlOptions};

#[cfg(feature = "graphql")]
mod commands;
#[cfg(feature = "graphql")]
mod complexity;
#[cfg(feature = "graphql")]
mod compile;
#[cfg(feature = "graphql")]
mod engine;
#[cfg(feature = "graphql")]
mod execute;
#[cfg(feature = "graphql")]
mod filter;
#[cfg(feature = "graphql")]
pub mod http;
#[cfg(feature = "graphql")]
pub mod identity;
#[cfg(feature = "graphql")]
mod permissions;
#[cfg(feature = "graphql")]
mod schema;
#[cfg(feature = "graphql")]
pub mod subscribe;
#[cfg(feature = "graphql")]
mod types;

#[cfg(feature = "graphql")]
pub use commands::{exposed_command, ExposedCommand, GraphqlCommands};
#[cfg(feature = "graphql")]
pub use engine::{
    graphiql_enabled_from_env, graphiql_enabled_from_env_vars, GraphqlBuildError, GraphqlEngine,
    GraphqlEngineBuilder, GraphqlPool,
};
#[cfg(feature = "graphql")]
pub use filter::{claim, col, lit, rel, ClaimRef, ColRef, FilterExpr, LitValue, Operand};
#[cfg(feature = "graphql")]
pub use http::{graphiql_page, graphql_router, graphql_router_with_service};
#[cfg(feature = "graphql")]
pub use identity::{
    extract_bearer, map_claims_to_session, resolve_session, resolve_session_sync,
    strip_identity_headers, AuthError, ClaimMapConfig, IdentityConfig, IdentityMode, OidcConfig,
    OidcValidator, TrustedProxyConfig, ValidationError, DEFAULT_IDENTITY_STRIP_HEADERS,
};
#[cfg(feature = "graphql")]
pub use permissions::{select, ModelPermissions, SelectPermission};
#[cfg(feature = "graphql")]
pub use subscribe::ChangeHub;
#[cfg(feature = "graphql")]
pub use types::{GraphqlInputType, GraphqlOutputType, GraphqlTypeDef, GraphqlTypeField};
