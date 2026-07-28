//! Auto-generated read-only GraphQL over relational read models.
//!
//! `naming` and `sdl` always compile (zero deps beyond the rest of the crate)
//! so `dctl schema --format graphql` works without enabling the `graphql`
//! feature. Execution, the dynamic schema, and the axum router sit behind
//! `feature = "graphql"`.

pub mod client_manifest;
pub(crate) mod command_contract;
pub mod naming;
pub mod sdl;
pub mod surface;

// These modules are structural GraphQL metadata and deliberately remain
// pool/server independent. `dctl` and the runtime engine must compile the same
// surface without pulling in async-graphql or a database adapter.
mod commands;
mod complexity_contract;
mod filter;
mod permissions;
mod types;

pub use client_manifest::*;
#[doc(hidden)]
pub use command_contract::{
    __command_confirmations, __command_effects, __command_input_defaults, __effect_assignment,
    __effect_constant, __effect_delete, __effect_input, __effect_invalidate_model,
    __effect_invalidate_relationship, __effect_key, __effect_key_assignment, __effect_key_field,
    __effect_link, __effect_null, __effect_patch, __effect_relationship, __effect_trusted,
    __effect_unlink, __effect_upsert, __input_default_ulid, __input_default_uuid_v7,
    CombineEffectNullability, CompiledEffectFieldValue, CompiledEffectKeyField,
    CompiledEffectOperation, CompiledInputDefault, EffectAssignmentExpression,
    EffectInputDescendableKind, EffectInputFieldMarker, EffectInputObjectKind, EffectInputPath,
    EffectInputPathKind, EffectInputTerminalKind, EffectModelFieldMarker, EffectNullable,
    EffectPathNullability, EffectRelationshipMarker, EffectRequired, EffectWireBigInt,
    EffectWireBoolean, EffectWireBytea, EffectWireChecked, EffectWireCompatible, EffectWireFloat,
    EffectWireJson, EffectWireList, EffectWireLiteral, EffectWireObject, EffectWireString,
    EffectWireTimestamp, EffectWireUnsupported,
};
pub use command_contract::{
    __command_projection_events, typed_command, Causal, CommandConsistency,
    CommandProjectionEventSet, CommandProjectionPreview, CommandProjectionPreviewSource,
    CompiledCommandEffects, CompiledConfirmationPlan, CompiledDirectProjectionTarget,
    CompiledInputDefaults, PrepareCommandError, PreparedCommand, Projected, Succeeded,
    TypedCommand, TypedEffectExpression, TypedEffectKey, TypedEffectRelationship,
};
pub use naming::{
    aggregate_field, by_pk_field, comparison_op_fields, include_postgres_json_comparison_ops,
    is_valid_graphql_name, object_type_name, root_list_field, scalar_type_name,
    PORTABLE_COMPARISON_OPS, POSTGRES_JSON_COMPARISON_OPS, STRING_COMPARISON_OPS,
};
pub use sdl::{
    graphql_sdl_for_role, graphql_sdl_for_tables, graphql_sdl_for_tables_with_options,
    graphql_sdl_from_surface, SdlOptions,
};
pub use surface::{
    build_surface, role_grants_for_role, surface_for_application, surface_for_role, RoleGrant,
    RootField, RootKind, Surface, SurfaceArgument, SurfaceArgumentKind, SurfaceCommand,
    SurfaceCommandShape, SurfaceDialect, SurfaceDirectProjection, SurfaceModel, SurfaceOptions,
    SurfaceProjectionOwner, SurfaceProjector, SurfaceRelationshipAggregate,
    SurfaceRelationshipKeys, SurfaceRowPolicy, SurfaceTypeDef, SurfaceTypeField,
};

pub use filter::{claim, col, lit, rel, ClaimRef, ColRef, FilterExpr, LitValue, Operand};
pub use permissions::{
    read, role_grant_from_read_permission, role_grants_from_model_role_perms, ModelPermissions,
    ReadPermission,
};
pub use types::{GraphqlInputType, GraphqlOutputType, GraphqlTypeDef, GraphqlTypeField};

#[cfg(feature = "graphql")]
pub(crate) mod command_input;
#[cfg(feature = "graphql")]
mod compile;
#[cfg(feature = "graphql")]
mod complexity;
#[cfg(feature = "graphql")]
mod engine;
#[cfg(feature = "graphql")]
mod execute;
#[cfg(feature = "graphql")]
pub mod http;
#[cfg(feature = "graphql")]
pub mod identity;
#[cfg(feature = "graphql")]
pub(crate) mod protocol;
#[cfg(feature = "graphql")]
pub(crate) mod query_protocol;
#[cfg(feature = "graphql")]
mod schema;
#[cfg(feature = "graphql")]
pub mod subscribe;
#[cfg(feature = "graphql")]
pub use engine::{
    graphiql_enabled_from_env, graphiql_enabled_from_env_vars, GraphqlBuildError, GraphqlEngine,
    GraphqlEngineBuilder, GraphqlPool, GraphqlPoolSource,
};
#[cfg(feature = "graphql")]
pub use http::{graphiql_page, graphql_router, graphql_router_with_service};
#[cfg(feature = "graphql")]
pub use identity::{
    extract_bearer, map_claims_to_session, public_oidc_identity_from_env,
    public_oidc_identity_from_env_vars, resolve_session, resolve_session_sync,
    strip_identity_headers, AuthError, ClaimMapConfig, IdentityConfig, IdentityMode,
    IdentityResolver, OidcConfig, OidcValidator, TrustedProxyConfig, ValidationError,
    DEFAULT_IDENTITY_STRIP_HEADERS, UNSET_OIDC_AUDIENCE, UNSET_OIDC_ISSUER,
};
#[cfg(feature = "graphql")]
pub use subscribe::ChangeHub;
