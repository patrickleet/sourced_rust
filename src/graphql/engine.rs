//! GraphqlEngine builder, validation, and execute entrypoint.
#![allow(clippy::items_after_test_module)]

use std::any::TypeId;
use std::collections::{btree_map::Entry, BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;

use async_graphql::{Request, Response, ServerError, Value};
use futures_util::stream::{self, BoxStream};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::manifest::DistributedProjectManifest;
use crate::microsvc::{Service, Session, ROLE_KEY, USER_ID_KEY};
use crate::read_model::{ReadModelChange, RelationalReadModelIncludes};
use crate::table::{
    resolve_m2m_target_foreign_key, ColumnType, RelationshipKind, TableKind, TableSchema,
};

use super::client_manifest::{
    trusted_preset_descriptors, ClientExecutionLimits, ClientManifestError, ClientSurfaceIdentity,
    ClientTrustedPresetDescriptor, DistributedClientManifest, DistributedClientSurfaceExport,
};
use super::command_contract::TypedServiceCommandBinding;
use super::commands::TypedCommandInventory;
use super::compile::{SqlDialect, SqlPlan};
use super::execute;
use super::filter::{validate_row_policy_operand_literal, FilterExpr, Operand};
use super::identity::{IdentityConfig, IdentityMode, OidcValidator, VerifiedPrincipal};
use super::naming::{
    by_pk_field, is_valid_graphql_name, object_type_name, reserved_type_names, root_list_field,
};
use super::permissions::{
    read, role_grants_from_model_role_perms, ModelPermissions, ReadPermission,
};
use super::protocol::{
    DistributedEnvelopeV2, DistributedLiveCursor, DistributedTrustedPreset, OpaqueProtocolToken,
    ProtocolResponseAccumulator, ProtocolTokenCodec, ProtocolTokenPurpose, RequestedLiveResume,
    MAX_LIVE_RESUME_CURSORS,
};
use super::query_protocol::QueryProtocolRuntime;
use super::schema as dyn_schema;
use super::sdl::{graphql_sdl_for_tables_with_options, graphql_sdl_from_surface, SdlOptions};
use super::surface::{
    build_surface, surface_for_application, surface_for_role, Surface, SurfaceDialect,
    SurfaceOptions, SurfaceProjectionOwner, SurfaceProjector, SurfaceSelection,
};

const GRAPHIQL_INTROSPECTION_MAX_DEPTH_FLOOR: usize = 15;
const GRAPHIQL_INTROSPECTION_MAX_COMPLEXITY_FLOOR: usize = 10_000;
const REQUEST_ANALYSIS_MAX_DEPTH: usize = 128;
const REQUEST_ANALYSIS_MAX_SELECTIONS: usize = 4_096;

#[derive(Clone)]
pub enum GraphqlPool {
    #[cfg(feature = "postgres")]
    Postgres(sqlx::PgPool),
    #[cfg(feature = "sqlite")]
    Sqlite(sqlx::SqlitePool),
}

#[cfg(feature = "postgres")]
impl From<sqlx::PgPool> for GraphqlPool {
    fn from(pool: sqlx::PgPool) -> Self {
        GraphqlPool::Postgres(pool)
    }
}

#[cfg(feature = "sqlite")]
impl From<sqlx::SqlitePool> for GraphqlPool {
    fn from(pool: sqlx::SqlitePool) -> Self {
        GraphqlPool::Sqlite(pool)
    }
}

/// Database source for a GraphQL engine.
///
/// Passing a Distributed repository handle instead of its raw pool preserves
/// the opaque storage identity required to prove that `Projected` commands
/// update the same database read by GraphQL.
#[derive(Clone)]
pub struct GraphqlPoolSource {
    pool: GraphqlPool,
    causal_storage_identity: Option<crate::command_ledger::CausalStorageIdentity>,
}

impl From<GraphqlPool> for GraphqlPoolSource {
    fn from(pool: GraphqlPool) -> Self {
        Self {
            pool,
            causal_storage_identity: None,
        }
    }
}

#[cfg(feature = "postgres")]
impl From<sqlx::PgPool> for GraphqlPoolSource {
    fn from(pool: sqlx::PgPool) -> Self {
        GraphqlPool::from(pool).into()
    }
}

#[cfg(feature = "sqlite")]
impl From<sqlx::SqlitePool> for GraphqlPoolSource {
    fn from(pool: sqlx::SqlitePool) -> Self {
        GraphqlPool::from(pool).into()
    }
}

#[cfg(feature = "postgres")]
impl From<&crate::PostgresRepository> for GraphqlPoolSource {
    fn from(repository: &crate::PostgresRepository) -> Self {
        Self {
            pool: GraphqlPool::Postgres(repository.pool().clone()),
            causal_storage_identity: Some(repository.causal_storage_identity()),
        }
    }
}

#[cfg(feature = "sqlite")]
impl From<&crate::SqliteRepository> for GraphqlPoolSource {
    fn from(repository: &crate::SqliteRepository) -> Self {
        Self {
            pool: GraphqlPool::Sqlite(repository.pool().clone()),
            causal_storage_identity: Some(repository.causal_storage_identity()),
        }
    }
}

#[cfg(all(test, feature = "sqlite"))]
mod graphql_pool_source_identity_tests {
    use super::GraphqlPoolSource;

    fn pool() -> sqlx::SqlitePool {
        sqlx::sqlite::SqlitePoolOptions::new()
            .connect_lazy("sqlite::memory:")
            .expect("lazy SQLite pool")
    }

    #[tokio::test]
    async fn raw_pool_has_no_causal_storage_identity() {
        let source = GraphqlPoolSource::from(pool());

        assert!(source.causal_storage_identity.is_none());
    }

    #[tokio::test]
    async fn repository_reference_carries_its_opaque_storage_identity() {
        let repository = crate::SqliteRepository::new(pool());
        let source = GraphqlPoolSource::from(&repository);

        assert_eq!(
            source.causal_storage_identity,
            Some(repository.causal_storage_identity())
        );
    }

    #[tokio::test]
    async fn repository_and_pool_source_clones_preserve_storage_identity() {
        let repository = crate::SqliteRepository::new(pool());
        let repository_clone = repository.clone();
        let source = GraphqlPoolSource::from(&repository);
        let source_clone = source.clone();

        assert_eq!(
            GraphqlPoolSource::from(&repository_clone).causal_storage_identity,
            source.causal_storage_identity
        );
        assert_eq!(
            source_clone.causal_storage_identity,
            source.causal_storage_identity
        );
    }

    #[tokio::test]
    async fn independent_repositories_over_the_same_pool_have_distinct_identities() {
        let pool = pool();
        let first = crate::SqliteRepository::new(pool.clone());
        let second = crate::SqliteRepository::new(pool);

        assert_ne!(
            GraphqlPoolSource::from(&first).causal_storage_identity,
            GraphqlPoolSource::from(&second).causal_storage_identity
        );
    }
}

#[derive(Debug)]
pub struct GraphqlBuildError(pub String);

impl std::fmt::Display for GraphqlBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for GraphqlBuildError {}

impl From<String> for GraphqlBuildError {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for GraphqlBuildError {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

#[derive(Clone)]
pub(crate) struct CatalogEntry {
    pub schema: TableSchema,
    pub exposed: bool,
}

#[derive(Clone)]
pub(crate) struct RoleModelPerm {
    pub permission: ReadPermission,
}

#[derive(Clone)]
struct ProtocolSurfaceInfo {
    schema_fingerprint: String,
    protocol_fingerprint: String,
    trusted_presets: Vec<ClientTrustedPresetDescriptor>,
}

#[derive(Clone)]
struct ProtocolRoleInfo {
    surface: ProtocolSurfaceInfo,
    authorization_fingerprint: String,
    claim_keys: Vec<String>,
}

#[derive(Clone)]
struct ProtocolApplicationInfo {
    roles: Vec<String>,
    surface: ProtocolSurfaceInfo,
}

#[derive(Clone)]
struct ProtocolRuntime {
    codec: ProtocolTokenCodec,
    namespace: String,
    service_id: String,
    roles: BTreeMap<String, ProtocolRoleInfo>,
    applications: BTreeMap<String, ProtocolApplicationInfo>,
}

pub(crate) struct EngineInner {
    /// Stable service identity used by client manifest hashes and cache scopes.
    /// Manifest-built engines populate this automatically; manual builders may
    /// opt in with [`GraphqlEngineBuilder::service_id`].
    pub service_id: Option<String>,
    pub command_binding: Option<TypedServiceCommandBinding>,
    pub causal_storage_identity: Option<crate::command_ledger::CausalStorageIdentity>,
    pub pool: GraphqlPool,
    pub catalog: BTreeMap<String, CatalogEntry>,
    pub by_table: BTreeMap<String, String>,
    pub permissions: BTreeMap<(String, String), RoleModelPerm>,
    pub roles: BTreeSet<String>,
    pub anonymous_role: String,
    pub default_limit: u64,
    pub max_limit: u64,
    pub max_depth: usize,
    #[allow(dead_code)]
    pub max_complexity: usize,
    pub max_in_list: usize,
    /// Max length of a single `_and` / `_or` list in client `where` (breadth DoS).
    pub max_bool_width: usize,
    /// When true (default), unknown/ungranted client `where` and `order_by`
    /// keys fail the request instead of soft-skipping.
    pub strict_where: bool,
    #[allow(dead_code)]
    pub introspection_for_anonymous: bool,
    pub statement_timeout: Duration,
    pub graphiql: bool,
    pub(crate) typed_commands: TypedCommandInventory,
    pub role_surfaces: BTreeMap<String, Arc<Surface>>,
    pub application_surfaces: BTreeMap<String, Arc<Surface>>,
    pub schemas: HashMap<String, async_graphql::dynamic::Schema>,
    /// Relaxed schemas selected only for pure introspection while GraphiQL is
    /// enabled. Application operations always use `schemas` and its exact
    /// manifest-fingerprinted execution limits.
    pub graphiql_schemas: HashMap<String, async_graphql::dynamic::Schema>,
    pub change_hub: super::subscribe::ChangeHub,
    pub dialect: SqlDialect,
    /// Identity mode for HTTP session construction (see `identity` module).
    pub identity: IdentityConfig,
    pub(crate) identity_validator: Option<OidcValidator>,
    protocol: Option<ProtocolRuntime>,
    pub(crate) query_protocol: QueryProtocolRuntime,
}

pub struct GraphqlEngine {
    pub(crate) inner: Arc<EngineInner>,
}

pub struct GraphqlEngineBuilder {
    service_id: Option<String>,
    protocol_token_key: Option<[u8; 32]>,
    protocol_namespace: Option<String>,
    client_applications: BTreeMap<String, Vec<String>>,
    command_binding: Option<TypedServiceCommandBinding>,
    causal_storage_identity: Option<crate::command_ledger::CausalStorageIdentity>,
    pool: GraphqlPool,
    catalog: BTreeMap<String, CatalogEntry>,
    by_table: BTreeMap<String, String>,
    permissions: BTreeMap<(String, String), RoleModelPerm>,
    roles: Option<BTreeSet<String>>,
    anonymous_role: String,
    default_limit: u64,
    max_limit: u64,
    max_depth: usize,
    max_complexity: usize,
    max_in_list: usize,
    max_bool_width: usize,
    strict_where: bool,
    introspection_for_anonymous: bool,
    statement_timeout: Duration,
    graphiql: bool,
    typed_commands: TypedCommandInventory,
    projectors: Vec<SurfaceProjectionOwner>,
    change_rx: Option<tokio::sync::broadcast::Receiver<ReadModelChange>>,
    pending_errors: Vec<String>,
    identity: IdentityConfig,
}

impl GraphqlEngine {
    pub fn builder(pool: impl Into<GraphqlPoolSource>) -> GraphqlEngineBuilder {
        GraphqlEngineBuilder::new(pool.into())
    }

    pub fn from_manifest(
        m: &DistributedProjectManifest,
        pool: impl Into<GraphqlPoolSource>,
    ) -> Result<GraphqlEngineBuilder, GraphqlBuildError> {
        let mut builder = Self::builder(pool).service_id(m.name.clone());
        for schema in &m.tables {
            if schema.kind == TableKind::ReadModel {
                builder = builder.register_schema_exposed(schema.clone())?;
            } else {
                // Operational tables never become roots, but the shared
                // Surface needs them to derive opaque m2m dependencies.
                builder = builder.table_schema(schema.clone());
            }
        }
        Ok(builder)
    }

    pub fn sdl_for_role(&self, role: &str) -> Option<String> {
        if !self.inner.roles.contains(role) && role != self.inner.anonymous_role {
            // Still allow any role that has a built schema.
        }
        self.inner.schemas.get(role).map(|s| s.sdl())
    }

    /// Dep-free **surface IR** role SDL (A11 production path).
    ///
    /// Preferred for deterministic SDL generation and schema-drift checks. Uses
    /// the same catalog grants as the engine build (A12 mapper). Runtime dump is
    /// still available via [`Self::sdl_for_role`].
    pub fn ir_sdl_for_role(&self, role: &str) -> Result<String, String> {
        let surface = self
            .inner
            .role_surfaces
            .get(role)
            .ok_or_else(|| format!("role `{role}` is not configured for GraphQL"))?;
        graphql_sdl_from_surface(surface)
    }

    /// Return the exact role Surface instance used to construct the runtime
    /// schema. This is intentionally shared rather than reconstructed.
    pub fn surface_for_role(&self, role: &str) -> Option<Arc<Surface>> {
        self.inner.role_surfaces.get(role).cloned()
    }

    /// Stable service identity retained from [`DistributedProjectManifest::name`].
    ///
    /// Engines built manually return `None` unless the builder opted in with
    /// [`GraphqlEngineBuilder::service_id`].
    pub fn service_id(&self) -> Option<&str> {
        self.inner.service_id.as_deref()
    }

    pub(crate) fn typed_command_binding(&self) -> Option<&TypedServiceCommandBinding> {
        self.inner.command_binding.as_ref()
    }

    pub(crate) fn typed_command_contracts_for_service(
        &self,
    ) -> Result<Vec<super::command_contract::TypedCommandContract>, String> {
        Ok(self.inner.typed_commands.contracts_for_binding())
    }

    pub(crate) fn causal_storage_identity(
        &self,
    ) -> Option<crate::command_ledger::CausalStorageIdentity> {
        self.inner.causal_storage_identity
    }

    pub(crate) fn causal_protocol_configured(&self) -> bool {
        self.inner.protocol.is_some()
    }

    pub fn client_surface_for_role(
        &self,
        role: &str,
    ) -> Result<DistributedClientSurfaceExport, ClientManifestError> {
        let service_id = self.client_export_service_id()?;
        let surface = self.surface_for_role(role).ok_or_else(|| {
            ClientManifestError(format!("role `{role}` is not configured for GraphQL"))
        })?;
        DistributedClientSurfaceExport::from_selected_with_execution(
            service_id,
            surface,
            ClientExecutionLimits::from_runtime(
                self.inner.max_depth,
                self.inner.max_complexity,
                self.inner.max_bool_width,
                self.inner.max_in_list,
            )?,
        )
    }

    pub fn client_manifest_for_role(
        &self,
        role: &str,
    ) -> Result<DistributedClientManifest, ClientManifestError> {
        self.client_surface_for_role(role)?.manifest()
    }

    pub fn client_surface_for_application(
        &self,
        application: &str,
        roles: &[&str],
    ) -> Result<DistributedClientSurfaceExport, ClientManifestError> {
        let service_id = self.client_export_service_id()?;
        let surface = self
            .inner
            .application_surfaces
            .get(application)
            .cloned()
            .ok_or_else(|| {
                ClientManifestError(format!(
                    "application surface `{application}` is not registered"
                ))
            })?;
        let mut requested_roles = roles
            .iter()
            .map(|role| (*role).to_string())
            .collect::<Vec<_>>();
        requested_roles.sort();
        requested_roles.dedup();
        let SurfaceSelection::Application {
            roles: registered_roles,
            ..
        } = &surface.selection
        else {
            return Err(ClientManifestError(format!(
                "registered application surface `{application}` has invalid identity"
            )));
        };
        if &requested_roles != registered_roles {
            return Err(ClientManifestError(format!(
                "application surface `{application}` is registered for roles [{}], not [{}]",
                registered_roles.join(", "),
                requested_roles.join(", ")
            )));
        }
        DistributedClientSurfaceExport::from_selected_with_execution(
            service_id,
            surface,
            ClientExecutionLimits::from_runtime(
                self.inner.max_depth,
                self.inner.max_complexity,
                self.inner.max_bool_width,
                self.inner.max_in_list,
            )?,
        )
    }

    pub fn client_manifest_for_application(
        &self,
        application: &str,
        roles: &[&str],
    ) -> Result<DistributedClientManifest, ClientManifestError> {
        self.client_surface_for_application(application, roles)?
            .manifest()
    }

    fn client_export_service_id(&self) -> Result<String, ClientManifestError> {
        self.inner.service_id.clone().ok_or_else(|| {
            ClientManifestError(
                "client export requires a service ID; construct the engine with GraphqlEngine::from_manifest or GraphqlEngineBuilder::service_id"
                    .into(),
            )
        })
    }

    pub fn graphiql_enabled(&self) -> bool {
        self.inner.graphiql
    }

    /// Identity configuration used by GraphQL HTTP handlers.
    pub fn identity_config(&self) -> &IdentityConfig {
        &self.inner.identity
    }

    pub(crate) fn identity_validator(&self) -> Option<&OidcValidator> {
        self.inner.identity_validator.as_ref()
    }

    /// Whether unknown/ungranted client `where` and `order_by` keys fail closed.
    /// Default is `true` (see [`GraphqlEngineBuilder::strict_where`]).
    pub fn strict_where(&self) -> bool {
        self.inner.strict_where
    }

    pub async fn execute(&self, session: &Session, mut request: Request) -> Response {
        let role = resolve_role(session, &self.inner.anonymous_role);
        let introspection = self.inner.graphiql && is_pure_introspection_request(&mut request);
        let schema = if introspection {
            self.inner
                .graphiql_schemas
                .get(&role)
                .or_else(|| self.inner.schemas.get(&role))
        } else {
            self.inner.schemas.get(&role)
        };
        let Some(schema) = schema else {
            return Response::from_errors(vec![ServerError::new(
                format!("role `{role}` is not configured for GraphQL"),
                None,
            )]);
        };
        if has_multiple_protocol_query_roots(&self.inner, &role, &mut request) {
            return protocol_multi_root_error_response();
        }

        let accumulator = match self.protocol_accumulator(&role, session, &request) {
            Ok(accumulator) => accumulator,
            Err(()) => return protocol_internal_error_response(),
        };
        if introspection {
            // The relaxed schema is defense-in-depth restricted even if a
            // future classifier or request extension behaves unexpectedly.
            request = request.only_introspection();
        }
        let mut request = request.data(session.clone()).data(Arc::clone(&self.inner));
        if let Some(accumulator) = &accumulator {
            request = request.data(accumulator.clone());
        }
        let start = std::time::Instant::now();
        let response =
            attach_protocol_response(schema.execute(request).await, accumulator.as_ref());
        let status = metrics_status_for_response(&response);
        let root_field = match &response.data {
            Value::Object(map) => map.keys().next().map(|s| s.as_str()).unwrap_or("_"),
            _ => "_",
        };
        record_metrics(session, root_field, status, start.elapsed());
        response
    }

    /// Hub used by live subscriptions (tests may publish directly).
    pub fn change_hub(&self) -> &super::subscribe::ChangeHub {
        &self.inner.change_hub
    }

    /// Execute a GraphQL subscription document as a stream of responses.
    pub fn execute_stream(
        &self,
        session: &Session,
        mut request: Request,
    ) -> BoxStream<'static, async_graphql::Response> {
        let role = resolve_role(session, &self.inner.anonymous_role);
        let introspection = self.inner.graphiql && is_pure_introspection_request(&mut request);
        let schema = if introspection {
            self.inner
                .graphiql_schemas
                .get(&role)
                .or_else(|| self.inner.schemas.get(&role))
        } else {
            self.inner.schemas.get(&role)
        };
        let Some(schema) = schema.cloned() else {
            return stream::once(async move {
                Response::from_errors(vec![ServerError::new(
                    format!("role `{role}` is not configured for GraphQL"),
                    None,
                )])
            })
            .boxed();
        };
        if has_multiple_protocol_query_roots(&self.inner, &role, &mut request) {
            return stream::once(async { protocol_multi_root_error_response() }).boxed();
        }
        let accumulator = match self.protocol_accumulator(&role, session, &request) {
            Ok(accumulator) => accumulator,
            Err(()) => {
                return stream::once(async { protocol_internal_error_response() }).boxed();
            }
        };
        if accumulator
            .as_ref()
            .is_some_and(|accumulator| accumulator.begin_stream().is_err())
        {
            return stream::once(async { protocol_internal_error_response() }).boxed();
        }
        if introspection {
            request = request.only_introspection();
        }
        let mut request = request
            .data(session.clone())
            .data(std::sync::Arc::clone(&self.inner));
        if let Some(accumulator) = &accumulator {
            request = request.data(accumulator.clone());
        }
        schema
            .execute_stream(request)
            .map(move |response| attach_protocol_response(response, accumulator.as_ref()))
            .boxed()
    }

    fn protocol_accumulator(
        &self,
        role: &str,
        session: &Session,
        request: &Request,
    ) -> Result<Option<ProtocolResponseAccumulator>, ()> {
        let Some(runtime) = &self.inner.protocol else {
            return Ok(None);
        };
        let role_info = runtime.roles.get(role).ok_or(())?;
        let (surface_identity, surface_info) =
            select_protocol_surface(runtime, role, request).map_err(|_| ())?;
        let trusted_presets = surface_info
            .trusted_presets
            .iter()
            .map(|descriptor| resolve_protocol_preset(session, descriptor).ok_or(()))
            .collect::<Result<Vec<_>, _>>()?;
        let principal = request
            .data
            .get(&TypeId::of::<VerifiedPrincipal>())
            .and_then(|principal| principal.downcast_ref::<VerifiedPrincipal>());
        let principal_partition =
            principal.map(|principal| principal.partition_for_service(&runtime.service_id));
        let session_authorization_context = role_info
            .claim_keys
            .iter()
            .map(|key| (key.as_str(), session.get(key)))
            .collect::<Vec<_>>();

        #[derive(Serialize)]
        struct CacheScopeMaterial<'a> {
            domain: &'static str,
            version: u32,
            namespace: &'a str,
            service_id: &'a str,
            role: &'a str,
            surface: &'a ClientSurfaceIdentity,
            schema_fingerprint: &'a str,
            protocol_fingerprint: &'a str,
            authorization_surface_fingerprint: &'a str,
            identity_mode: &'static str,
            verified_principal_partition: Option<&'a str>,
            session_authorization_context: Vec<(&'a str, Option<&'a str>)>,
            trusted_presets: &'a [DistributedTrustedPreset],
        }

        // Only session values that can affect authorization enter the HMAC:
        // role/user plus claim keys referenced by this role's row policies.
        // Ambient headers such as cookies or user-agent must not churn caches.
        // Raw values and the verified principal partition remain private HMAC
        // inputs and are never echoed in the response.
        let material = CacheScopeMaterial {
            domain: "distributed.graphql.cache-scope",
            version: 1,
            namespace: &runtime.namespace,
            service_id: &runtime.service_id,
            role,
            surface: &surface_identity,
            schema_fingerprint: &surface_info.schema_fingerprint,
            protocol_fingerprint: &surface_info.protocol_fingerprint,
            authorization_surface_fingerprint: &role_info.authorization_fingerprint,
            identity_mode: identity_mode_label(self.inner.identity.mode),
            verified_principal_partition: principal_partition.as_deref(),
            session_authorization_context,
            trusted_presets: &trusted_presets,
        };
        let cache_scope = runtime
            .codec
            .issue(ProtocolTokenPurpose::CacheScope, &material)
            .map_err(|_| ())?;
        let envelope = DistributedEnvelopeV2::new(
            surface_info.schema_fingerprint.clone(),
            cache_scope,
            // Generated artifacts submit this exact document. Hashing its
            // bytes matches manifest operation_hash and provides a useful
            // identity/drift fence without claiming APQ negotiation.
            Some(operation_fingerprint(&request.query)),
        )
        .with_trusted_presets(trusted_presets);
        let accumulator = ProtocolResponseAccumulator::new(envelope, runtime.codec.clone());
        accumulator
            .set_requested_live_resume(parse_requested_live_resume(request))
            .map_err(|_| ())?;
        Ok(Some(accumulator))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RequestedProtocolClient {
    surface: ClientSurfaceIdentity,
    schema_hash: String,
}

fn requested_protocol_client(request: &Request) -> Result<Option<RequestedProtocolClient>, ()> {
    let Some(distributed) = request.extensions.get("distributed") else {
        return Ok(None);
    };
    let distributed = distributed.clone().into_json().map_err(|_| ())?;
    let distributed = distributed.as_object().ok_or(())?;
    let Some(client) = distributed.get("client") else {
        return Ok(None);
    };
    serde_json::from_value(client.clone())
        .map(Some)
        .map_err(|_| ())
}

fn select_protocol_surface<'a>(
    runtime: &'a ProtocolRuntime,
    role: &str,
    request: &Request,
) -> Result<(ClientSurfaceIdentity, &'a ProtocolSurfaceInfo), ()> {
    let Some(requested) = requested_protocol_client(request)? else {
        let info = runtime.roles.get(role).ok_or(())?;
        return Ok((ClientSurfaceIdentity::role(role), &info.surface));
    };
    match requested.surface {
        ClientSurfaceIdentity::Role { name } => {
            if name != role {
                return Err(());
            }
            let info = runtime.roles.get(&name).ok_or(())?;
            if requested.schema_hash != info.surface.schema_fingerprint {
                return Err(());
            }
            Ok((ClientSurfaceIdentity::role(name), &info.surface))
        }
        ClientSurfaceIdentity::Application { name, roles } => {
            let application = runtime.applications.get(&name).ok_or(())?;
            if roles != application.roles
                || roles
                    .binary_search_by(|candidate| candidate.as_str().cmp(role))
                    .is_err()
                || requested.schema_hash != application.surface.schema_fingerprint
            {
                return Err(());
            }
            Ok((
                ClientSurfaceIdentity::application(name, roles),
                &application.surface,
            ))
        }
    }
}

fn resolve_protocol_preset(
    session: &Session,
    descriptor: &ClientTrustedPresetDescriptor,
) -> Option<DistributedTrustedPreset> {
    use base64::Engine as _;

    // Match SQL row-policy claim lookup exactly: applications commonly
    // normalize HTTP header names to lowercase before constructing Session.
    let raw = session
        .get(&descriptor.name)
        .or_else(|| session.get(&descriptor.name.to_ascii_lowercase()))?;
    let value = match descriptor.codec.as_str() {
        "string" | "string_unvalidated_timestamp" => serde_json::Value::String(raw.to_string()),
        "base64" => {
            let decoded = base64::engine::general_purpose::STANDARD.decode(raw).ok()?;
            if base64::engine::general_purpose::STANDARD.encode(decoded) != raw {
                return None;
            }
            serde_json::Value::String(raw.to_string())
        }
        "boolean" => match raw {
            "true" => serde_json::Value::Bool(true),
            "false" => serde_json::Value::Bool(false),
            _ => return None,
        },
        "int32" => {
            let parsed = raw.parse::<i32>().ok()?;
            if parsed.to_string() != raw {
                return None;
            }
            serde_json::Value::Number(parsed.into())
        }
        "json_number_precision_limited" => {
            let parsed = raw.parse::<i64>().ok()?;
            if !(-9_007_199_254_740_991..=9_007_199_254_740_991).contains(&parsed)
                || parsed.to_string() != raw
            {
                return None;
            }
            serde_json::Value::Number(parsed.into())
        }
        "float64" => {
            let parsed = raw.parse::<f64>().ok()?;
            if !parsed.is_finite() {
                return None;
            }
            serde_json::Value::Number(serde_json::Number::from_f64(parsed)?)
        }
        "json" => serde_json::from_str(raw).ok()?,
        _ => return None,
    };
    Some(DistributedTrustedPreset {
        name: descriptor.name.clone(),
        codec: descriptor.codec.clone(),
        value,
    })
}

fn protocol_trusted_presets(
    manifest: &DistributedClientManifest,
) -> Result<Vec<ClientTrustedPresetDescriptor>, GraphqlBuildError> {
    trusted_preset_descriptors(manifest).map_err(|error| GraphqlBuildError(error.to_string()))
}

fn operation_fingerprint(document: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(document.as_bytes()))
}

/// True only when the selected query operation contains introspection root
/// fields and nothing application-owned.
///
/// GraphiQL needs a deeper schema-introspection budget than application
/// operations. Selection is deliberately fail-closed: mixed roots, missing or
/// recursive fragments, over-budget documents, ambiguous operations,
/// mutations, and subscriptions all stay on the normal
/// manifest-fingerprinted schema.
fn is_pure_introspection_request(request: &mut Request) -> bool {
    if request.introspection_mode == async_graphql::IntrospectionMode::Disabled {
        return false;
    }
    let operation_name = request.operation_name.clone();
    // `Request::set_parsed_query` is public and async-graphql executes that
    // cached AST when present. Inspect (and, for ordinary requests, populate)
    // the exact AST execution will consume rather than reparsing `query`.
    let Ok(document) = request.parsed_query() else {
        return false;
    };
    let mut operations = document.operations.iter();
    let operation = if let Some(requested) = operation_name.as_deref() {
        operations
            .find(|(name, _)| name.map(|name| name.as_str()) == Some(requested))
            .map(|(_, operation)| operation)
    } else {
        let first = operations.next().map(|(_, operation)| operation);
        if operations.next().is_some() {
            None
        } else {
            first
        }
    };
    let Some(operation) = operation else {
        return false;
    };
    if operation.node.ty != async_graphql::parser::types::OperationType::Query {
        return false;
    }

    fn selection_is_introspection_only(
        selection: &async_graphql::parser::types::SelectionSet,
        document: &async_graphql::parser::types::ExecutableDocument,
        visiting: &mut BTreeSet<String>,
        completed: &mut HashMap<String, bool>,
        remaining_selections: &mut usize,
        depth: usize,
    ) -> bool {
        if depth > REQUEST_ANALYSIS_MAX_DEPTH
            || selection.items.is_empty()
            || selection.items.len() > *remaining_selections
        {
            return false;
        }
        *remaining_selections -= selection.items.len();
        selection.items.iter().all(|item| match &item.node {
            async_graphql::parser::types::Selection::Field(field) => matches!(
                field.node.name.node.as_str(),
                "__schema" | "__type" | "__typename"
            ),
            async_graphql::parser::types::Selection::InlineFragment(fragment) => {
                selection_is_introspection_only(
                    &fragment.node.selection_set.node,
                    document,
                    visiting,
                    completed,
                    remaining_selections,
                    depth + 1,
                )
            }
            async_graphql::parser::types::Selection::FragmentSpread(spread) => {
                let name = spread.node.fragment_name.node.to_string();
                if let Some(valid) = completed.get(&name) {
                    return *valid;
                }
                if !visiting.insert(name.clone()) {
                    return false;
                }
                let valid = document
                    .fragments
                    .get(&spread.node.fragment_name.node)
                    .is_some_and(|fragment| {
                        selection_is_introspection_only(
                            &fragment.node.selection_set.node,
                            document,
                            visiting,
                            completed,
                            remaining_selections,
                            depth + 1,
                        )
                    });
                visiting.remove(&name);
                completed.insert(name, valid);
                valid
            }
        })
    }

    let mut remaining_selections = REQUEST_ANALYSIS_MAX_SELECTIONS;
    selection_is_introspection_only(
        &operation.node.selection_set.node,
        &document,
        &mut BTreeSet::new(),
        &mut HashMap::new(),
        &mut remaining_selections,
        0,
    )
}

/// Until the query executor owns an operation-wide database transaction, two
/// independent read roots cannot truthfully share one causal snapshot. Fail
/// closed instead of merging separately observed rows and duplicate index
/// vectors into an envelope that generated clients would treat as atomic.
fn has_multiple_protocol_query_roots(
    inner: &EngineInner,
    role: &str,
    request: &mut Request,
) -> bool {
    if inner.protocol.is_none() {
        return false;
    }
    let Some(surface) = inner.role_surfaces.get(role) else {
        return false;
    };
    let query_roots = surface
        .query_root_names()
        .into_iter()
        .collect::<BTreeSet<_>>();
    if query_roots.is_empty() {
        return false;
    }

    let operation_name = request.operation_name.clone();
    let Ok(document) = request.parsed_query() else {
        return false;
    };
    let mut operations = document.operations.iter();
    let operation = if let Some(requested) = operation_name.as_deref() {
        operations
            .find(|(name, _)| name.map(|name| name.as_str()) == Some(requested))
            .map(|(_, operation)| operation)
    } else {
        let first = operations.next().map(|(_, operation)| operation);
        if operations.next().is_some() {
            None
        } else {
            first
        }
    };
    let Some(operation) = operation else {
        return false;
    };
    if operation.node.ty != async_graphql::parser::types::OperationType::Query {
        return false;
    }

    fn collect_root_keys<'a>(
        selection: &'a async_graphql::parser::types::SelectionSet,
        document: &'a async_graphql::parser::types::ExecutableDocument,
        query_roots: &BTreeSet<&str>,
        visiting: &mut BTreeSet<String>,
        completed: &mut BTreeSet<String>,
        remaining_selections: &mut usize,
        response_keys: &mut BTreeSet<String>,
        depth: usize,
    ) -> Result<(), ()> {
        if response_keys.len() > 1 {
            return Ok(());
        }
        if depth > REQUEST_ANALYSIS_MAX_DEPTH || selection.items.len() > *remaining_selections {
            return Err(());
        }
        *remaining_selections -= selection.items.len();
        for item in &selection.items {
            match &item.node {
                async_graphql::parser::types::Selection::Field(field) => {
                    if query_roots.contains(field.node.name.node.as_str()) {
                        response_keys.insert(field.node.response_key().node.to_string());
                    }
                }
                async_graphql::parser::types::Selection::InlineFragment(fragment) => {
                    collect_root_keys(
                        &fragment.node.selection_set.node,
                        document,
                        query_roots,
                        visiting,
                        completed,
                        remaining_selections,
                        response_keys,
                        depth + 1,
                    )?;
                }
                async_graphql::parser::types::Selection::FragmentSpread(spread) => {
                    let name = spread.node.fragment_name.node.to_string();
                    if completed.contains(&name) {
                        continue;
                    }
                    if !visiting.insert(name.clone()) {
                        return Err(());
                    }
                    let Some(fragment) = document.fragments.get(&spread.node.fragment_name.node)
                    else {
                        return Err(());
                    };
                    let result = collect_root_keys(
                        &fragment.node.selection_set.node,
                        document,
                        query_roots,
                        visiting,
                        completed,
                        remaining_selections,
                        response_keys,
                        depth + 1,
                    );
                    visiting.remove(&name);
                    result?;
                    completed.insert(name);
                }
            }
            if response_keys.len() > 1 {
                return Ok(());
            }
        }
        Ok(())
    }

    let mut response_keys = BTreeSet::new();
    let mut remaining_selections = REQUEST_ANALYSIS_MAX_SELECTIONS;
    let analysis = collect_root_keys(
        &operation.node.selection_set.node,
        &document,
        &query_roots,
        &mut BTreeSet::new(),
        &mut BTreeSet::new(),
        &mut remaining_selections,
        &mut response_keys,
        0,
    );
    analysis.is_err() || response_keys.len() > 1
}

fn protocol_multi_root_error_response() -> Response {
    Response::from_errors(vec![ServerError::new(
        "causal GraphQL operations currently support one read root so data and revision evidence share one atomic snapshot; split this operation into separate requests",
        None,
    )])
}

const MAX_LIVE_RESUME_PROJECTION_BYTES: usize = 512;

/// Parse the private request extension used by generated live operations.
/// Invalid input is a conservative reset signal, never trusted cursor state.
fn parse_requested_live_resume(request: &Request) -> RequestedLiveResume {
    let Some(distributed) = request.extensions.get("distributed") else {
        return RequestedLiveResume::Absent;
    };
    let Ok(distributed) = distributed.clone().into_json() else {
        return RequestedLiveResume::Invalid;
    };
    let Some(distributed) = distributed.as_object() else {
        return RequestedLiveResume::Invalid;
    };
    let Some(resume) = distributed.get("resume") else {
        return RequestedLiveResume::Absent;
    };
    let Some(cursors) = resume
        .as_object()
        .and_then(|resume| resume.get("cursors"))
        .and_then(serde_json::Value::as_array)
    else {
        return RequestedLiveResume::Invalid;
    };
    if cursors.len() > MAX_LIVE_RESUME_CURSORS {
        return RequestedLiveResume::Invalid;
    }

    let mut parsed = Vec::with_capacity(cursors.len());
    for cursor in cursors {
        let Some(cursor) = cursor.as_object() else {
            return RequestedLiveResume::Invalid;
        };
        let Some(projection) = cursor.get("projection").and_then(serde_json::Value::as_str) else {
            return RequestedLiveResume::Invalid;
        };
        if projection.is_empty() || projection.len() > MAX_LIVE_RESUME_PROJECTION_BYTES {
            return RequestedLiveResume::Invalid;
        }
        let Some(position) = cursor.get("position").and_then(serde_json::Value::as_str) else {
            return RequestedLiveResume::Invalid;
        };
        let Ok(parsed_position) = position.parse::<u64>() else {
            return RequestedLiveResume::Invalid;
        };
        if parsed_position.to_string() != position {
            return RequestedLiveResume::Invalid;
        }
        let Some(token) = cursor.get("token").and_then(serde_json::Value::as_str) else {
            return RequestedLiveResume::Invalid;
        };
        let Ok(token) = OpaqueProtocolToken::parse(token) else {
            return RequestedLiveResume::Invalid;
        };
        parsed.push(DistributedLiveCursor {
            projection: projection.to_string(),
            position: position.to_string(),
            token,
        });
    }
    RequestedLiveResume::Cursors(parsed)
}

#[cfg(test)]
mod live_resume_request_tests {
    use super::*;

    fn request_with_resume(value: serde_json::Value) -> Request {
        serde_json::from_value(serde_json::json!({
            "query": "subscription Watch { todos { id } }",
            "extensions": { "distributed": { "resume": value } }
        }))
        .expect("GraphQL request")
    }

    #[test]
    fn live_resume_request_is_bounded_and_canonical() {
        let token = ProtocolTokenCodec::new([9; 32])
            .issue(ProtocolTokenPurpose::LiveResume, &("bounded-test", 7_u64))
            .unwrap();
        let request = request_with_resume(serde_json::json!({
            "cursors": [{
                "projection": "todos",
                "position": "7",
                "token": token.as_str()
            }]
        }));
        let RequestedLiveResume::Cursors(cursors) = parse_requested_live_resume(&request) else {
            panic!("valid cursor must parse")
        };
        assert_eq!(cursors.len(), 1);
        assert_eq!(cursors[0].projection, "todos");
        assert_eq!(cursors[0].position, "7");

        for invalid in [
            serde_json::json!({"cursors": [{
                "projection": "todos", "position": "07", "token": token.as_str()
            }]}),
            serde_json::json!({"cursors": [{
                "projection": "todos", "position": "7", "token": "not-a-token"
            }]}),
            serde_json::json!({"cursors": "not-an-array"}),
        ] {
            assert_eq!(
                parse_requested_live_resume(&request_with_resume(invalid)),
                RequestedLiveResume::Invalid
            );
        }

        let too_many = vec![
            serde_json::json!({
                "projection": "todos",
                "position": "7",
                "token": token.as_str()
            });
            MAX_LIVE_RESUME_CURSORS + 1
        ];
        assert_eq!(
            parse_requested_live_resume(&request_with_resume(
                serde_json::json!({"cursors": too_many})
            )),
            RequestedLiveResume::Invalid
        );
    }

    #[test]
    fn request_without_resume_remains_a_fresh_subscription() {
        let request: Request = serde_json::from_value(serde_json::json!({
            "query": "subscription Watch { todos { id } }",
            "extensions": { "distributed": {} }
        }))
        .unwrap();
        assert_eq!(
            parse_requested_live_resume(&request),
            RequestedLiveResume::Absent
        );
    }
}

fn role_authorization_info(
    role: &str,
    permissions: &BTreeMap<(String, String), RoleModelPerm>,
) -> Result<(String, Vec<String>), GraphqlBuildError> {
    #[derive(Serialize)]
    struct PermissionMaterial<'a> {
        model: &'a str,
        all_columns: bool,
        columns: Vec<&'a str>,
        row_filter: Option<&'a FilterExpr>,
        limit: Option<u64>,
        aggregations: bool,
    }

    #[derive(Serialize)]
    struct RoleAuthorizationMaterial<'a> {
        domain: &'static str,
        version: u32,
        role: &'a str,
        permissions: Vec<PermissionMaterial<'a>>,
    }

    let mut claim_keys = BTreeSet::from([ROLE_KEY.to_string(), USER_ID_KEY.to_string()]);
    let mut role_permissions = Vec::new();
    for ((model, permission_role), entry) in permissions {
        if permission_role != role {
            continue;
        }
        if let Some(filter) = &entry.permission.row_filter {
            collect_filter_claim_keys(filter, &mut claim_keys);
        }
        role_permissions.push(PermissionMaterial {
            model,
            all_columns: entry.permission.all_columns,
            columns: entry
                .permission
                .columns
                .as_ref()
                .map(|columns| columns.iter().map(String::as_str).collect())
                .unwrap_or_default(),
            row_filter: entry.permission.row_filter.as_ref(),
            limit: entry.permission.limit,
            aggregations: entry.permission.aggregations,
        });
    }
    let canonical = serde_json::to_vec(&RoleAuthorizationMaterial {
        domain: "distributed.graphql.authorization-surface",
        version: 1,
        role,
        permissions: role_permissions,
    })
    .map_err(|_| {
        GraphqlBuildError(format!(
            "failed to encode GraphQL authorization surface for role `{role}`"
        ))
    })?;
    Ok((
        format!("sha256:{:x}", Sha256::digest(canonical)),
        claim_keys.into_iter().collect(),
    ))
}

fn collect_filter_claim_keys(filter: &FilterExpr, keys: &mut BTreeSet<String>) {
    fn collect_operand(operand: &Operand, keys: &mut BTreeSet<String>) {
        if let Operand::Claim(claim) = operand {
            keys.insert(claim.header.clone());
        }
    }

    match filter {
        FilterExpr::And(items) | FilterExpr::Or(items) => {
            for item in items {
                collect_filter_claim_keys(item, keys);
            }
        }
        FilterExpr::Not(item)
        | FilterExpr::Rel {
            predicate: item, ..
        } => {
            collect_filter_claim_keys(item, keys);
        }
        FilterExpr::Cmp { rhs, .. } => collect_operand(rhs, keys),
        FilterExpr::In { values, .. } => {
            for value in values {
                collect_operand(value, keys);
            }
        }
        FilterExpr::IsNull { .. } => {}
    }
}

fn identity_mode_label(mode: IdentityMode) -> &'static str {
    match mode {
        IdentityMode::TrustedProxy => "trusted_proxy",
        IdentityMode::OidcBearer => "oidc_bearer",
        IdentityMode::Hybrid => "hybrid",
        IdentityMode::DevHeaders => "dev_headers",
    }
}

fn protocol_internal_error_response() -> Response {
    Response::from_errors(vec![ServerError::new(
        "internal protocol response error",
        None,
    )])
}

fn attach_protocol_response(
    mut response: Response,
    accumulator: Option<&ProtocolResponseAccumulator>,
) -> Response {
    let Some(accumulator) = accumulator else {
        return response;
    };
    if accumulator.attach(&mut response).is_ok() {
        return response;
    }

    // A resolver cannot shadow the framework-owned extension. Replace a
    // colliding response with a closed internal error and attach the one
    // authoritative envelope.
    let mut failure = protocol_internal_error_response();
    if accumulator.attach(&mut failure).is_ok() {
        failure
    } else {
        protocol_internal_error_response()
    }
}

fn resolve_role(session: &Session, anonymous: &str) -> String {
    session
        .role()
        .filter(|r| !r.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| anonymous.to_string())
}

/// Map GraphQL response errors to coarse metric `status` labels.
///
/// Privacy: only stable class names (`ok`, `timeout`, `bad_request`,
/// `forbidden`, `internal`, `error`) — never user/tenant/SQL text.
pub(crate) fn metrics_status_for_response(response: &Response) -> &'static str {
    if !response.is_err() {
        return "ok";
    }
    for err in &response.errors {
        if let Some(ext) = &err.extensions {
            if let Some(code) = ext.get("code") {
                let code = format!("{code:?}").to_ascii_uppercase();
                if code.contains("TIMEOUT") {
                    return "timeout";
                }
                if code.contains("BAD_REQUEST") {
                    return "bad_request";
                }
                if code.contains("FORBIDDEN") {
                    return "forbidden";
                }
                if code.contains("INTERNAL") {
                    return "internal";
                }
            }
        }
        let msg = err.message.to_ascii_lowercase();
        if msg.contains("timeout") {
            return "timeout";
        }
        if msg.contains("not configured") || msg.contains("forbidden") {
            return "forbidden";
        }
    }
    "error"
}

fn record_metrics(session: &Session, root_field: &str, status: &str, duration: Duration) {
    let _ = session;
    #[cfg(feature = "metrics")]
    crate::metrics::record_graphql_request(None, root_field, status, duration);
    #[cfg(not(feature = "metrics"))]
    let _ = (root_field, status, duration);
}

impl GraphqlEngineBuilder {
    fn new(source: GraphqlPoolSource) -> Self {
        Self {
            service_id: None,
            protocol_token_key: None,
            protocol_namespace: None,
            client_applications: BTreeMap::new(),
            command_binding: None,
            causal_storage_identity: source.causal_storage_identity,
            pool: source.pool,
            catalog: BTreeMap::new(),
            by_table: BTreeMap::new(),
            permissions: BTreeMap::new(),
            roles: None,
            anonymous_role: "anonymous".into(),
            default_limit: 100,
            max_limit: 1000,
            max_depth: super::complexity::DEFAULT_MAX_DEPTH,
            max_complexity: super::complexity::DEFAULT_MAX_COMPLEXITY,
            max_in_list: 1000,
            max_bool_width: 256,
            // Fail-closed by default for unshipped GA: unknown/ungranted filter
            // and order keys must not silently no-op.
            strict_where: true,
            introspection_for_anonymous: true,
            statement_timeout: Duration::from_secs(5),
            graphiql: false,
            typed_commands: TypedCommandInventory::empty(),
            projectors: Vec::new(),
            change_rx: None,
            pending_errors: Vec::new(),
            // DevHeaders keeps ambient header tests/green; public scaffolds set OidcBearer (D6).
            identity: IdentityConfig::dev_headers(),
        }
    }

    pub fn model<M: RelationalReadModelIncludes>(mut self, perms: ModelPermissions<M>) -> Self {
        let schema = M::schema().clone();
        if let Err(e) = self.insert_catalog(schema.clone(), true) {
            self.pending_errors.push(e.0);
            return self;
        }
        // Shadow-register one-hop relationship targets.
        for rel in &schema.relationships {
            match M::include_target_schema(&rel.field_name) {
                Ok(target) => {
                    if let Err(e) = self.insert_catalog(target.clone(), false) {
                        // Shadow insert conflict only if different schema.
                        if !e.0.contains("identical") {
                            self.pending_errors.push(e.0);
                        }
                    }
                }
                Err(err) => {
                    self.pending_errors.push(format!(
                        "include_target_schema for `{}`.`{}`: {err}",
                        schema.model_name, rel.field_name
                    ));
                }
            }
        }
        for (role, perm) in perms.entries {
            let key = (schema.model_name.clone(), role.clone());
            match self.permissions.entry(key) {
                Entry::Vacant(entry) => {
                    entry.insert(RoleModelPerm { permission: perm });
                }
                Entry::Occupied(_) => {
                    self.pending_errors.push(format!(
                        "duplicate permission for model `{}` role `{role}`",
                        schema.model_name
                    ));
                }
            }
        }
        self
    }

    pub fn table_schema(mut self, schema: TableSchema) -> Self {
        if let Err(e) = self.insert_catalog(schema, false) {
            self.pending_errors.push(e.0);
        }
        self
    }

    fn register_schema_exposed(mut self, schema: TableSchema) -> Result<Self, GraphqlBuildError> {
        self.insert_catalog(schema, true)?;
        Ok(self)
    }

    fn insert_catalog(
        &mut self,
        schema: TableSchema,
        exposed: bool,
    ) -> Result<(), GraphqlBuildError> {
        schema
            .validate()
            .map_err(|e| GraphqlBuildError(e.to_string()))?;
        if let Some(existing) = self.by_table.get(&schema.table_name) {
            if existing != &schema.model_name {
                return Err(GraphqlBuildError(format!(
                    "duplicate table_name `{}` for models `{existing}` and `{}`",
                    schema.table_name, schema.model_name
                )));
            }
        }
        match self.catalog.get(&schema.model_name) {
            Some(entry) if entry.schema != schema => {
                return Err(GraphqlBuildError(format!(
                    "conflicting schemas for model `{}`",
                    schema.model_name
                )));
            }
            Some(entry) => {
                // Identical: upgrade shadow → exposed if needed.
                if exposed && !entry.exposed {
                    let mut upgraded = entry.clone();
                    upgraded.exposed = true;
                    self.catalog.insert(schema.model_name.clone(), upgraded);
                }
                return Ok(());
            }
            None => {}
        }
        self.by_table
            .insert(schema.table_name.clone(), schema.model_name.clone());
        self.catalog
            .insert(schema.model_name.clone(), CatalogEntry { schema, exposed });
        Ok(())
    }

    pub fn roles(mut self, roles: &[&str]) -> Self {
        self.roles = Some(roles.iter().map(|r| (*r).to_string()).collect());
        self
    }

    pub fn grant_all(mut self, role: &str) -> Self {
        let exposed: Vec<String> = self
            .catalog
            .iter()
            .filter(|(_, e)| e.exposed)
            .map(|(n, _)| n.clone())
            .collect();
        for model in exposed {
            let key = (model.clone(), role.to_string());
            match self.permissions.entry(key) {
                Entry::Vacant(entry) => {
                    entry.insert(RoleModelPerm {
                        permission: read().all_columns().aggregations(),
                    });
                }
                Entry::Occupied(_) => {
                    self.pending_errors.push(format!(
                        "duplicate permission for model `{model}` role `{role}`"
                    ));
                }
            }
        }
        self
    }

    pub fn permission<M: RelationalReadModelIncludes>(
        mut self,
        role: &str,
        p: ReadPermission,
    ) -> Self {
        let model = M::schema().model_name.clone();
        if !self.catalog.contains_key(&model) {
            self.pending_errors.push(format!(
                "permission for unregistered model `{model}` (role `{role}`)"
            ));
            return self;
        }
        let key = (model.clone(), role.to_string());
        match self.permissions.entry(key) {
            Entry::Vacant(entry) => {
                entry.insert(RoleModelPerm { permission: p });
            }
            Entry::Occupied(_) => {
                self.pending_errors.push(format!(
                    "duplicate permission for model `{model}` role `{role}`"
                ));
            }
        }
        self
    }

    pub fn anonymous_role(mut self, name: &str) -> Self {
        self.anonymous_role = name.to_string();
        self
    }
    /// Set the stable service identity used by generated client manifests.
    ///
    /// [`GraphqlEngine::from_manifest`] supplies this automatically from the
    /// project manifest. This setter is intended for manually assembled
    /// engines, which otherwise cannot export a client manifest safely.
    pub fn service_id(mut self, service_id: impl Into<String>) -> Self {
        let service_id = service_id.into();
        if service_id.trim().is_empty() {
            self.pending_errors
                .push("GraphQL client service ID must not be empty".into());
            return self;
        }
        if let Some(binding) = &self.command_binding {
            if binding.service_id != service_id {
                self.pending_errors.push(format!(
                    "GraphQL service ID `{service_id}` does not match bound executable service ID `{}`",
                    binding.service_id
                ));
                return self;
            }
        }
        if let Some(existing) = self.service_id.as_deref() {
            if existing != service_id {
                self.pending_errors.push(format!(
                    "GraphQL service ID was already configured as `{existing}` and cannot be changed to `{service_id}`"
                ));
                return self;
            }
        }
        self.service_id = Some(service_id);
        self
    }

    /// Configure the stable deployment key used for opaque GraphQL protocol
    /// tokens.
    ///
    /// All replicas serving the same endpoint namespace must receive the same
    /// key, and rotations intentionally create new cache/projection scopes.
    /// The key is never exposed to resolvers or serialized to clients.
    pub fn protocol_token_key(mut self, key: [u8; 32]) -> Self {
        if key.iter().all(|byte| *byte == 0) {
            self.pending_errors
                .push("GraphQL protocol token key must not be all zero".into());
            return self;
        }
        self.protocol_token_key = Some(key);
        self
    }

    /// Optionally isolate protocol tokens for one stable public endpoint.
    ///
    /// When omitted, the engine's service ID is the namespace. This is useful
    /// when the same service exposes independently deployed GraphQL surfaces.
    pub fn protocol_namespace(mut self, namespace: impl Into<String>) -> Self {
        let namespace = namespace.into();
        if namespace.trim().is_empty() {
            self.pending_errors
                .push("GraphQL protocol namespace must not be empty".into());
            return self;
        }
        if namespace.len() > 255 || namespace.chars().any(char::is_control) {
            self.pending_errors.push(
                "GraphQL protocol namespace must be at most 255 bytes and contain no control characters"
                    .into(),
            );
            return self;
        }
        self.protocol_namespace = Some(namespace);
        self
    }

    /// Register one exact named application surface for generated clients.
    ///
    /// Runtime requests may select only this frozen name/role set. The server
    /// still authorizes every request as its verified concrete role; the
    /// application surface controls only the schema generation presented to
    /// the client.
    pub fn client_application_surface(
        mut self,
        application: impl Into<String>,
        roles: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        let application = application.into();
        let mut roles = roles.into_iter().map(Into::into).collect::<Vec<String>>();
        roles.sort();
        roles.dedup();
        if application.is_empty()
            || application.len() > 128
            || application.trim() != application
            || application.chars().any(char::is_control)
        {
            self.pending_errors.push(
                "GraphQL client application name must be 1..=128 bytes, have no surrounding whitespace, and contain no control characters"
                    .into(),
            );
            return self;
        }
        if roles.is_empty()
            || roles.iter().any(|role| {
                role.is_empty()
                    || role.len() > 128
                    || role.trim() != role
                    || role.chars().any(char::is_control)
            })
        {
            self.pending_errors.push(format!(
                "GraphQL client application `{application}` must declare one or more bounded non-empty roles"
            ));
            return self;
        }
        if self
            .client_applications
            .insert(application.clone(), roles)
            .is_some()
        {
            self.pending_errors.push(format!(
                "GraphQL client application `{application}` was registered more than once"
            ));
        }
        self
    }

    /// Derive GraphQL command mutations from the service's typed executable
    /// inventory. This is the authoritative path: no second command list is
    /// accepted, and attachment later verifies the complete structural digest
    /// plus exact Rust input/output `TypeId`s.
    pub fn service(mut self, service: &Service) -> Self {
        if self.command_binding.is_some() {
            self.pending_errors
                .push("GraphQL service command inventory was configured more than once".into());
            return self;
        }
        let binding = match service.typed_command_binding() {
            Ok(binding) => binding,
            Err(error) => {
                self.pending_errors.push(error);
                return self;
            }
        };
        if let Some(configured) = self.service_id.as_deref() {
            if configured != binding.service_id {
                self.pending_errors.push(format!(
                    "GraphQL service ID `{configured}` does not match executable service ID `{}`",
                    binding.service_id
                ));
                return self;
            }
        }
        let contracts = service.typed_command_contracts();
        match TypedCommandInventory::from_contracts(&contracts) {
            Ok(commands) => self.typed_commands = commands,
            Err(error) => {
                self.pending_errors.push(error);
                return self;
            }
        }
        self.service_id = Some(binding.service_id.clone());
        self.command_binding = Some(binding);
        self
    }
    pub fn default_limit(mut self, n: u64) -> Self {
        self.default_limit = n;
        self
    }
    pub fn max_limit(mut self, n: u64) -> Self {
        self.max_limit = n;
        self
    }
    pub fn max_depth(mut self, n: usize) -> Self {
        self.max_depth = n;
        self
    }
    /// Maximum nested-selection complexity budget (default
    /// [`super::complexity::DEFAULT_MAX_COMPLEXITY`]).
    ///
    /// Cost uses relationship-aware weights (see `complexity` module), not only
    /// flat GraphQL field counts, so multi-level `has_many` trees are bounded.
    pub fn max_complexity(mut self, n: usize) -> Self {
        self.max_complexity = n;
        self
    }
    pub fn max_in_list(mut self, n: usize) -> Self {
        self.max_in_list = n;
        self
    }
    /// Cap width of a single `_and` / `_or` list in client `where` (default 256).
    pub fn max_bool_width(mut self, n: usize) -> Self {
        self.max_bool_width = n;
        self
    }
    /// Fail closed on unknown or ungranted client `where` / `order_by` keys.
    ///
    /// **Default: `true`.** Set `false` only for intentional Hasura-style
    /// soft-skip of unknown keys (not recommended for production).
    pub fn strict_where(mut self, on: bool) -> Self {
        self.strict_where = on;
        self
    }
    pub fn introspection_for_anonymous(mut self, on: bool) -> Self {
        self.introspection_for_anonymous = on;
        self
    }
    /// Declare projector topology for client invalidation planning. The model
    /// and dependency IDs are validated when the one shared Surface is built.
    pub fn client_projectors(
        mut self,
        projectors: impl IntoIterator<Item = SurfaceProjector>,
    ) -> Self {
        self.projectors = projectors.into_iter().map(Into::into).collect();
        self
    }

    /// Declare a mixed client/query registry of async projectors and
    /// same-transaction-only projection owners.
    pub fn client_projection_owners(
        mut self,
        projectors: impl IntoIterator<Item = SurfaceProjectionOwner>,
    ) -> Self {
        self.projectors = projectors.into_iter().collect();
        self
    }
    pub fn statement_timeout(mut self, d: Duration) -> Self {
        self.statement_timeout = d;
        self
    }
    /// Enable the GraphiQL IDE on `GET /graphql`.
    ///
    /// GraphiQL's full introspection query receives an isolated depth/complexity
    /// allowance. Enabling an operational IDE never changes application query
    /// limits, generated client manifests, or schema fingerprints.
    pub fn graphiql(mut self, on: bool) -> Self {
        self.graphiql = on;
        self
    }
    /// Configure GraphQL HTTP identity mode (TrustedProxy / OidcBearer / Hybrid / DevHeaders).
    pub fn identity(mut self, config: IdentityConfig) -> Self {
        self.identity = config;
        self
    }
    pub fn change_stream(mut self, rx: tokio::sync::broadcast::Receiver<ReadModelChange>) -> Self {
        self.change_rx = Some(rx);
        self
    }

    pub fn build(mut self) -> Result<GraphqlEngine, GraphqlBuildError> {
        if !self.pending_errors.is_empty() {
            return Err(GraphqlBuildError(self.pending_errors.join("; ")));
        }
        if self.protocol_namespace.is_some() && self.protocol_token_key.is_none() {
            return Err(GraphqlBuildError(
                "GraphQL protocol namespace requires a protocol token key".into(),
            ));
        }
        if self.protocol_token_key.is_some() && self.service_id.is_none() {
            return Err(GraphqlBuildError(
                "GraphQL protocol tokens require a stable service ID; construct the engine with GraphqlEngine::from_manifest or GraphqlEngineBuilder::service_id"
                    .into(),
            ));
        }

        let model_schemas = self
            .catalog
            .iter()
            .map(|(model, entry)| (model.clone(), entry.schema.clone()))
            .collect::<BTreeMap<_, _>>();
        self.typed_commands
            .bind_direct_projection_targets(&self.projectors, &model_schemas)
            .map_err(GraphqlBuildError)?;

        if let Some(expected) = self.command_binding.as_ref() {
            let service_id = self.service_id.as_deref().ok_or_else(|| {
                GraphqlBuildError(
                    "bound typed command inventory is missing its GraphQL service ID".into(),
                )
            })?;
            let contracts = self.typed_commands.contracts_for_binding();
            let actual = TypedServiceCommandBinding::from_contracts(service_id, &contracts)
                .map_err(GraphqlBuildError)?;
            // The builder copied this exact private command inventory from the
            // service and prevents later replacement. Its only intentional
            // structural change is the framework-owned direct-owner binding
            // above, so retain the pre-bind exact Rust type proof and publish
            // the final bound fingerprint for Service attachment.
            if actual.service_id != expected.service_id || actual.types != expected.types {
                return Err(GraphqlBuildError(
                    "final GraphQL command inventory differs from the bound executable service inventory"
                        .into(),
                ));
            }
            self.command_binding = Some(actual);
        }

        // Validate permissions.
        let declared_roles = self.roles.clone();
        let anonymous = self.anonymous_role.clone();
        let perm_keys: Vec<_> = self.permissions.keys().cloned().collect();
        for (model, role) in &perm_keys {
            if let Some(roles) = &declared_roles {
                if !roles.contains(role) && role != &anonymous {
                    return Err(GraphqlBuildError(format!(
                        "permission for undeclared role `{role}` (model `{model}`)"
                    )));
                }
            }
            let entry = self.catalog.get(model).ok_or_else(|| {
                GraphqlBuildError(format!("permission for unknown model `{model}`"))
            })?;
            let perm = &self.permissions[&(model.clone(), role.clone())].permission;

            // Column allowlist validation.
            if !perm.all_columns {
                if let Some(cols) = &perm.columns {
                    for col in cols {
                        if !entry
                            .schema
                            .columns
                            .iter()
                            .any(|c| c.column_name == *col && !c.skipped)
                        {
                            return Err(GraphqlBuildError(format!(
                                "unknown column `{col}` in permission for `{model}` role `{role}`"
                            )));
                        }
                    }
                }
            }

            if let Some(filter) = &perm.row_filter {
                validate_filter(
                    filter,
                    &entry.schema,
                    &self.catalog,
                    role == &anonymous,
                    model,
                    role,
                )?;
            }
        }

        // Name grammar / collisions across exposed models.
        validate_generated_names(&self.catalog)?;

        // m2m resolution for catalog relationships used in permissions/schema.
        for entry in self.catalog.values() {
            for rel in &entry.schema.relationships {
                if matches!(rel.kind, RelationshipKind::ManyToMany) {
                    if rel.through.is_none() {
                        return Err(GraphqlBuildError(format!(
                            "model `{}` relationship `{}` many-to-many must declare `through`",
                            entry.schema.model_name, rel.field_name
                        )));
                    }
                    if let (Some(target), Some(through_name)) =
                        (self.catalog.get(&rel.target_model), rel.through.as_deref())
                    {
                        if let Some(through_model) = self.by_table.get(through_name) {
                            if let Some(through) = self.catalog.get(through_model) {
                                resolve_m2m_target_foreign_key(
                                    &entry.schema,
                                    rel,
                                    &through.schema,
                                    &target.schema,
                                )
                                .map_err(|e| GraphqlBuildError(e.to_string()))?;
                            }
                        }
                    }
                }
            }
        }

        let dialect = match &self.pool {
            #[cfg(feature = "postgres")]
            GraphqlPool::Postgres(_) => SqlDialect::Postgres,
            #[cfg(feature = "sqlite")]
            GraphqlPool::Sqlite(_) => SqlDialect::Sqlite,
            #[allow(unreachable_patterns)]
            _ => {
                return Err(GraphqlBuildError(
                    "GraphqlPool has no store feature enabled".into(),
                ));
            }
        };

        let tables: Vec<TableSchema> = self
            .catalog
            .values()
            .map(|entry| entry.schema.clone())
            .collect();
        let surface_options = SurfaceOptions {
            dialect: match dialect {
                SqlDialect::Sqlite => SurfaceDialect::Sqlite,
                SqlDialect::Postgres => SurfaceDialect::Postgres,
            },
            aggregates: true,
            subscriptions: true,
            default_limit: self.default_limit,
            max_limit: self.max_limit,
        };
        let full_surface = Arc::new(
            build_surface(&tables, &surface_options)
                .map_err(GraphqlBuildError)?
                .with_typed_commands(&self.typed_commands)
                .map_err(GraphqlBuildError)?
                .with_service_binding(self.command_binding.clone())
                .with_projection_owners(self.projectors.clone())
                .map_err(GraphqlBuildError)?,
        );
        let query_protocol = if self.protocol_token_key.is_some() {
            QueryProtocolRuntime::compile(&full_surface).map_err(GraphqlBuildError)?
        } else {
            QueryProtocolRuntime::default()
        };

        let mut roles: BTreeSet<String> = self.permissions.keys().map(|(_, r)| r.clone()).collect();
        if let Some(declared) = &declared_roles {
            roles.extend(declared.iter().cloned());
        }
        roles.insert(anonymous.clone());

        // Build per-role dynamic schemas.
        let mut schemas = HashMap::new();
        let mut graphiql_schemas = HashMap::new();
        let mut role_surfaces = BTreeMap::new();
        let mut protocol_roles = BTreeMap::new();
        for role in &roles {
            let grants = role_grants_from_model_role_perms(
                role,
                self.permissions
                    .iter()
                    .map(|(key, value)| (key, &value.permission)),
            );
            let role_surface = Arc::new(
                surface_for_role(&full_surface, role, &grants).map_err(GraphqlBuildError)?,
            );
            let schema = dyn_schema::build_role_schema(
                &role_surface,
                self.max_depth,
                self.max_complexity,
                role == &anonymous && !self.introspection_for_anonymous,
            )
            .map_err(GraphqlBuildError)?;
            if self.graphiql {
                let graphiql_schema = dyn_schema::build_role_schema(
                    &role_surface,
                    self.max_depth.max(GRAPHIQL_INTROSPECTION_MAX_DEPTH_FLOOR),
                    self.max_complexity
                        .max(GRAPHIQL_INTROSPECTION_MAX_COMPLEXITY_FLOOR),
                    role == &anonymous && !self.introspection_for_anonymous,
                )
                .map_err(GraphqlBuildError)?;
                graphiql_schemas.insert(role.clone(), graphiql_schema);
            }
            if self.protocol_token_key.is_some() {
                let service_id = self
                    .service_id
                    .as_deref()
                    .expect("protocol configuration validated a service ID");
                let (authorization_fingerprint, claim_keys) =
                    role_authorization_info(role, &self.permissions)?;
                let manifest = DistributedClientSurfaceExport::from_selected_with_execution(
                    service_id,
                    Arc::clone(&role_surface),
                    ClientExecutionLimits::from_runtime(
                        self.max_depth,
                        self.max_complexity,
                        self.max_bool_width,
                        self.max_in_list,
                    )
                    .map_err(|error| GraphqlBuildError(error.to_string()))?,
                )
                .and_then(|export| export.manifest())
                .map_err(|error| {
                    GraphqlBuildError(format!(
                        "failed to derive GraphQL protocol surface for role `{role}`: {error}"
                    ))
                })?;
                let trusted_presets = protocol_trusted_presets(&manifest)?;
                protocol_roles.insert(
                    role.clone(),
                    ProtocolRoleInfo {
                        surface: ProtocolSurfaceInfo {
                            schema_fingerprint: manifest.schema_fingerprint,
                            protocol_fingerprint: manifest.protocol_fingerprint,
                            trusted_presets,
                        },
                        authorization_fingerprint,
                        claim_keys,
                    },
                );
            }
            schemas.insert(role.clone(), schema);
            role_surfaces.insert(role.clone(), role_surface);
        }

        let mut application_surfaces = BTreeMap::new();
        let mut protocol_applications = BTreeMap::new();
        for (application, application_roles) in &self.client_applications {
            let mut grants_by_role = BTreeMap::new();
            for role in application_roles {
                if !role_surfaces.contains_key(role) {
                    return Err(GraphqlBuildError(format!(
                        "client application surface `{application}` references unconfigured role `{role}`"
                    )));
                }
                grants_by_role.insert(
                    role.clone(),
                    role_grants_from_model_role_perms(
                        role,
                        self.permissions
                            .iter()
                            .map(|(key, value)| (key, &value.permission)),
                    ),
                );
            }
            let application_surface = Arc::new(
                surface_for_application(
                    &full_surface,
                    application,
                    application_roles,
                    &grants_by_role,
                )
                .map_err(GraphqlBuildError)?,
            );
            if self.protocol_token_key.is_some() {
                let service_id = self
                    .service_id
                    .as_deref()
                    .expect("protocol configuration validated a service ID");
                let manifest = DistributedClientSurfaceExport::from_selected_with_execution(
                    service_id,
                    Arc::clone(&application_surface),
                    ClientExecutionLimits::from_runtime(
                        self.max_depth,
                        self.max_complexity,
                        self.max_bool_width,
                        self.max_in_list,
                    )
                    .map_err(|error| GraphqlBuildError(error.to_string()))?,
                )
                .and_then(|export| export.manifest())
                .map_err(|error| {
                    GraphqlBuildError(format!(
                        "failed to derive GraphQL protocol surface for application `{application}`: {error}"
                    ))
                })?;
                let trusted_presets = protocol_trusted_presets(&manifest)?;
                protocol_applications.insert(
                    application.clone(),
                    ProtocolApplicationInfo {
                        roles: application_roles.clone(),
                        surface: ProtocolSurfaceInfo {
                            schema_fingerprint: manifest.schema_fingerprint,
                            protocol_fingerprint: manifest.protocol_fingerprint,
                            trusted_presets,
                        },
                    },
                );
            }
            application_surfaces.insert(application.clone(), application_surface);
        }

        let change_hub = super::subscribe::ChangeHub::new();
        if let Some(rx) = self.change_rx {
            super::subscribe::spawn_change_forwarder(change_hub.clone(), rx);
        }

        let protocol = self.protocol_token_key.map(|key| {
            let service_id = self
                .service_id
                .clone()
                .expect("protocol configuration validated a service ID");
            ProtocolRuntime {
                codec: ProtocolTokenCodec::new(key),
                namespace: self
                    .protocol_namespace
                    .unwrap_or_else(|| service_id.clone()),
                service_id,
                roles: protocol_roles,
                applications: protocol_applications,
            }
        });
        let identity_validator = self.identity.oidc.clone().map(OidcValidator::new);
        let inner = Arc::new(EngineInner {
            service_id: self.service_id,
            command_binding: self.command_binding,
            causal_storage_identity: self.causal_storage_identity,
            pool: self.pool,
            catalog: self.catalog,
            by_table: self.by_table,
            permissions: self.permissions,
            roles,
            anonymous_role: anonymous,
            default_limit: self.default_limit,
            max_limit: self.max_limit,
            max_depth: self.max_depth,
            max_complexity: self.max_complexity,
            max_in_list: self.max_in_list,
            max_bool_width: self.max_bool_width,
            strict_where: self.strict_where,
            introspection_for_anonymous: self.introspection_for_anonymous,
            statement_timeout: self.statement_timeout,
            graphiql: self.graphiql,
            typed_commands: self.typed_commands,
            role_surfaces,
            application_surfaces,
            schemas,
            graphiql_schemas,
            change_hub,
            dialect,
            identity: self.identity,
            identity_validator,
            protocol,
            query_protocol,
        });

        Ok(GraphqlEngine { inner })
    }
}

/// Resolve whether GraphiQL should be enabled from environment variables.
///
/// Policy (scaffold + operators):
/// - `GRAPHIQL` if set: on unless value is `0` / `false` / `off` / `no` (case-insensitive)
/// - else: **off** when `RUST_ENV` / `ENV` / `APP_ENV` is `production` or `prod`
/// - else: **on** (local/dev default)
///
/// Pure inputs so tests do not mutate process env. See [`graphiql_enabled_from_env`].
pub fn graphiql_enabled_from_env_vars(
    graphiql: Option<&str>,
    rust_env: Option<&str>,
    env: Option<&str>,
    app_env: Option<&str>,
) -> bool {
    if let Some(v) = graphiql {
        return !matches!(
            v.to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        );
    }
    let prod = rust_env
        .or(env)
        .or(app_env)
        .unwrap_or("")
        .to_ascii_lowercase();
    !matches!(prod.as_str(), "production" | "prod")
}

/// Read process env and apply [`graphiql_enabled_from_env_vars`].
pub fn graphiql_enabled_from_env() -> bool {
    graphiql_enabled_from_env_vars(
        std::env::var("GRAPHIQL").ok().as_deref(),
        std::env::var("RUST_ENV").ok().as_deref(),
        std::env::var("ENV").ok().as_deref(),
        std::env::var("APP_ENV").ok().as_deref(),
    )
}

#[cfg(test)]
mod graphiql_env_tests {
    use super::graphiql_enabled_from_env_vars;

    #[test]
    fn production_rust_env_disables_graphiql() {
        assert!(!graphiql_enabled_from_env_vars(
            None,
            Some("production"),
            None,
            None
        ));
        assert!(!graphiql_enabled_from_env_vars(
            None,
            Some("prod"),
            None,
            None
        ));
        assert!(!graphiql_enabled_from_env_vars(
            None,
            Some("PRODUCTION"),
            None,
            None
        ));
    }

    #[test]
    fn non_production_enables_graphiql_by_default() {
        assert!(graphiql_enabled_from_env_vars(
            None,
            Some("development"),
            None,
            None
        ));
        assert!(graphiql_enabled_from_env_vars(None, None, None, None));
    }

    #[test]
    fn explicit_graphiql_overrides_production() {
        assert!(graphiql_enabled_from_env_vars(
            Some("1"),
            Some("production"),
            None,
            None
        ));
        assert!(!graphiql_enabled_from_env_vars(
            Some("0"),
            Some("development"),
            None,
            None
        ));
        assert!(!graphiql_enabled_from_env_vars(
            Some("false"),
            None,
            None,
            None
        ));
    }

    #[test]
    fn env_and_app_env_aliases() {
        assert!(!graphiql_enabled_from_env_vars(
            None,
            None,
            Some("production"),
            None
        ));
        assert!(!graphiql_enabled_from_env_vars(
            None,
            None,
            None,
            Some("prod")
        ));
    }
}

#[cfg(test)]
mod metrics_status_tests {
    use super::metrics_status_for_response;
    use async_graphql::{ErrorExtensionValues, Response, ServerError};

    fn response_with_code(code: &str, message: &str) -> Response {
        let mut err = ServerError::new(message, None);
        let mut ext = ErrorExtensionValues::default();
        ext.set("code", code);
        err.extensions = Some(ext);
        Response::from_errors(vec![err])
    }

    #[test]
    fn ok_when_no_errors() {
        let resp = Response::new(async_graphql::Value::Null);
        assert_eq!(metrics_status_for_response(&resp), "ok");
    }

    #[test]
    fn maps_extension_codes() {
        assert_eq!(
            metrics_status_for_response(&response_with_code("TIMEOUT", "statement timeout")),
            "timeout"
        );
        assert_eq!(
            metrics_status_for_response(&response_with_code("BAD_REQUEST", "bad request")),
            "bad_request"
        );
        assert_eq!(
            metrics_status_for_response(&response_with_code("INTERNAL", "internal error")),
            "internal"
        );
        assert_eq!(
            metrics_status_for_response(&response_with_code("FORBIDDEN", "nope")),
            "forbidden"
        );
    }

    #[test]
    fn maps_message_fallback_timeout() {
        let resp = Response::from_errors(vec![ServerError::new("statement timeout", None)]);
        assert_eq!(metrics_status_for_response(&resp), "timeout");
    }
}

fn validate_generated_names(
    catalog: &BTreeMap<String, CatalogEntry>,
) -> Result<(), GraphqlBuildError> {
    let mut names: BTreeSet<String> = reserved_type_names().map(str::to_string).collect();
    for entry in catalog.values().filter(|e| e.exposed) {
        let schema = &entry.schema;
        for name in [
            object_type_name(schema).to_string(),
            root_list_field(schema).to_string(),
            by_pk_field(schema),
            format!("{}_bool_exp", schema.table_name),
            format!("{}_order_by", schema.table_name),
            format!("{}_aggregate", schema.table_name),
        ] {
            if !is_valid_graphql_name(&name) {
                return Err(GraphqlBuildError(format!(
                    "generated name `{name}` is not a valid GraphQL name"
                )));
            }
            if !names.insert(name.clone()) {
                return Err(GraphqlBuildError(format!(
                    "generated name `{name}` collides with another type or field"
                )));
            }
        }
    }
    Ok(())
}

fn validate_filter(
    filter: &FilterExpr,
    schema: &TableSchema,
    catalog: &BTreeMap<String, CatalogEntry>,
    is_anonymous: bool,
    model: &str,
    role: &str,
) -> Result<(), GraphqlBuildError> {
    filter.validate_row_policy_literals().map_err(|error| {
        GraphqlBuildError(format!(
            "invalid row policy for model `{model}` role `{role}`: {error}"
        ))
    })?;
    if is_anonymous {
        let mut claims = Vec::new();
        filter.visit_claims(|c| claims.push(c.to_string()));
        if !claims.is_empty() {
            return Err(GraphqlBuildError(format!(
                "claim() is not allowed in anonymous role filters (model `{model}`, claims: {})",
                claims.join(", ")
            )));
        }
    }

    filter.visit_columns(|col| {
        let _ = col;
    });
    // Re-walk for proper error returns.
    validate_filter_inner(filter, schema, catalog, model, role)
}

fn validate_filter_inner(
    filter: &FilterExpr,
    schema: &TableSchema,
    catalog: &BTreeMap<String, CatalogEntry>,
    model: &str,
    role: &str,
) -> Result<(), GraphqlBuildError> {
    match filter {
        FilterExpr::And(xs) | FilterExpr::Or(xs) => {
            for x in xs {
                validate_filter_inner(x, schema, catalog, model, role)?;
            }
        }
        FilterExpr::Not(x) => validate_filter_inner(x, schema, catalog, model, role)?,
        FilterExpr::Cmp { column, op, rhs } => {
            let col = schema
                .columns
                .iter()
                .find(|c| c.column_name == *column)
                .ok_or_else(|| {
                    GraphqlBuildError(format!(
                        "unknown column `{column}` in filter for `{model}` role `{role}`"
                    ))
                })?;
            if matches!(col.column_type, ColumnType::Json) && matches!(rhs, Operand::Claim(_)) {
                return Err(GraphqlBuildError(format!(
                    "claims cannot compare to Json columns (`{column}` on `{model}`)"
                )));
            }
            validate_row_policy_operand_literal(column, &col.column_type, Some(*op), rhs).map_err(
                |error| {
                    GraphqlBuildError(format!(
                        "invalid row policy for model `{model}` role `{role}`: {error}"
                    ))
                },
            )?;
        }
        FilterExpr::In { column, values, .. } => {
            let col = schema
                .columns
                .iter()
                .find(|candidate| candidate.column_name == *column)
                .ok_or_else(|| {
                    GraphqlBuildError(format!(
                        "unknown column `{column}` in filter for `{model}` role `{role}`"
                    ))
                })?;
            for (index, value) in values.iter().enumerate() {
                validate_row_policy_operand_literal(column, &col.column_type, None, value)
                    .map_err(|error| {
                        GraphqlBuildError(format!(
                            "invalid row policy for model `{model}` role `{role}` IN operand {index}: {error}"
                        ))
                    })?;
            }
        }
        FilterExpr::IsNull { column, .. } => {
            if !schema.columns.iter().any(|c| c.column_name == *column) {
                return Err(GraphqlBuildError(format!(
                    "unknown column `{column}` in filter for `{model}` role `{role}`"
                )));
            }
        }
        FilterExpr::Rel { field, predicate } => {
            let rel = schema
                .relationships
                .iter()
                .find(|r| r.field_name == *field)
                .ok_or_else(|| {
                    GraphqlBuildError(format!(
                        "rel(`{field}`) is not a relationship on model `{model}`"
                    ))
                })?;
            let target = catalog.get(&rel.target_model).ok_or_else(|| {
                GraphqlBuildError(format!(
                    "rel(`{field}`) target `{}` is not in the catalog (model `{model}`)",
                    rel.target_model
                ))
            })?;
            if matches!(rel.kind, RelationshipKind::ManyToMany) {
                let through = rel.through.as_deref().ok_or_else(|| {
                    GraphqlBuildError(format!(
                        "rel(`{field}`) many-to-many missing through on `{model}`"
                    ))
                })?;
                let through_model = catalog
                    .values()
                    .find(|e| e.schema.table_name == through)
                    .ok_or_else(|| {
                        GraphqlBuildError(format!(
                            "rel(`{field}`) through table `{through}` not in catalog"
                        ))
                    })?;
                let _ = through_model;
            }
            validate_filter_inner(predicate, &target.schema, catalog, &rel.target_model, role)?;
        }
    }
    Ok(())
}

/// Execute a compiled plan against the engine pool (used by root resolvers).
pub(crate) async fn execute_plan(inner: &EngineInner, plan: &SqlPlan) -> Result<Value, String> {
    execute::execute_sql(inner, plan).await
}

/// Public helper for tests: compile + naming surface.
#[allow(dead_code)]
pub fn core_sdl_for_catalog(tables: &[TableSchema]) -> Result<String, String> {
    // Dialect-independent / SQLite-default SDL (no PG JSON ops).
    graphql_sdl_for_tables_with_options(tables, &SdlOptions::sqlite())
}

#[cfg(all(test, any(feature = "sqlite", feature = "postgres")))]
mod client_surface_parity_tests {
    use std::any::TypeId;
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Arc;

    use sha2::{Digest, Sha256};

    use super::*;
    use crate::graphql::command_contract::{CommandEffects, TypedCommandContract};
    use crate::graphql::commands::TypedCommandInventory;
    #[cfg(feature = "sqlite")]
    use crate::graphql::ModelNormalization;
    use crate::graphql::{
        claim, col, ClientRootOperation, CommandConsistency, DistributedClientSurfaceExport,
        GraphqlInputType, GraphqlOutputType, GraphqlTypeDef, GraphqlTypeField, RoleGrant,
    };
    #[cfg(feature = "sqlite")]
    use crate::table::RelationshipDef;
    use crate::table::{ColumnType, PrimaryKey, TableColumn, TableKind, TableSchema};

    fn orders() -> TableSchema {
        TableSchema {
            model_name: "OrderView".into(),
            table_name: "orders".into(),
            columns: vec![
                TableColumn {
                    primary_key: true,
                    ..TableColumn::new("order_id", "order_id", ColumnType::Text)
                },
                TableColumn::new("status", "status", ColumnType::Text),
                TableColumn {
                    jsonb: true,
                    ..TableColumn::new("metadata", "metadata", ColumnType::Json)
                },
            ],
            primary_key: PrimaryKey::new(["order_id"]),
            version_column: Some("_sourced_version".into()),
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships: Vec::new(),
            kind: TableKind::ReadModel,
        }
    }

    fn duplicated_introspection_fragment_dag(depth: usize) -> String {
        let mut document = String::from("query Introspection { ...F0 }\n");
        for index in 0..depth {
            document.push_str(&format!(
                "fragment F{index} on Query {{ ...F{} ...F{} }}\n",
                index + 1,
                index + 1
            ));
        }
        document.push_str(&format!("fragment F{depth} on Query {{ __typename }}\n"));
        document
    }

    fn test_command<I, O>(
        command_name: &str,
        field_name: &str,
        roles: &[&str],
    ) -> TypedCommandContract
    where
        I: GraphqlInputType + 'static,
        O: GraphqlOutputType + 'static,
    {
        TypedCommandContract {
            name: command_name.into(),
            field_name: field_name.into(),
            roles: roles.iter().map(|role| (*role).into()).collect(),
            input: I::graphql_type().with_type_id(TypeId::of::<I>()),
            output: O::graphql_type().with_type_id(TypeId::of::<O>()),
            input_type_id: TypeId::of::<I>(),
            output_type_id: TypeId::of::<O>(),
            consistency: CommandConsistency::Accepted,
            input_defaults: Vec::new(),
            effects: CommandEffects::revalidate(),
            confirmations: Vec::new(),
            projected_model: None,
            direct_projection: None,
        }
    }

    fn test_service_binding(
        service_id: &str,
        commands: &TypedCommandInventory,
    ) -> TypedServiceCommandBinding {
        TypedServiceCommandBinding::from_contracts(service_id, &commands.contracts_for_binding())
            .unwrap()
    }

    fn type_field_names(sdl: &str, type_name: &str) -> BTreeSet<String> {
        definition_field_names(sdl, "type", type_name)
    }

    fn input_field_names(sdl: &str, type_name: &str) -> BTreeSet<String> {
        definition_field_names(sdl, "input", type_name)
    }

    fn definition_field_names(sdl: &str, declaration: &str, type_name: &str) -> BTreeSet<String> {
        let marker = format!("{declaration} {type_name} {{");
        let body = sdl
            .split_once(&marker)
            .unwrap_or_else(|| panic!("missing `{marker}` in SDL:\n{sdl}"))
            .1
            .split_once('}')
            .expect("type block should close")
            .0;
        body.lines()
            .filter_map(|line| {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    return None;
                }
                line.split(['(', ':'])
                    .next()
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_string)
            })
            .collect()
    }

    #[cfg(feature = "sqlite")]
    fn protocol_engine(namespace: &str) -> GraphqlEngine {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect_lazy("sqlite::memory:")
            .unwrap();
        let project = DistributedProjectManifest::new("orders-service").table_schema(orders());
        GraphqlEngine::from_manifest(&project, pool)
            .unwrap()
            .roles(&["user"])
            .grant_all("user")
            .protocol_token_key([7; 32])
            .protocol_namespace(namespace)
            .build()
            .unwrap()
    }

    #[cfg(feature = "sqlite")]
    fn policy_protocol_engine(namespace: &str, claim_key: &str) -> GraphqlEngine {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect_lazy("sqlite::memory:")
            .unwrap();
        let project = DistributedProjectManifest::new("orders-service").table_schema(orders());
        let mut builder = GraphqlEngine::from_manifest(&project, pool)
            .unwrap()
            .roles(&["user"])
            .grant_all("user");
        builder
            .permissions
            .get_mut(&("OrderView".into(), "user".into()))
            .unwrap()
            .permission
            .row_filter = Some(col("status").eq(claim(claim_key)));
        builder
            .protocol_token_key([7; 32])
            .protocol_namespace(namespace)
            .build()
            .unwrap()
    }

    #[cfg(feature = "sqlite")]
    fn preset_protocol_engine() -> GraphqlEngine {
        let mut engine = protocol_engine("preset-test");
        Arc::get_mut(&mut engine.inner)
            .expect("test owns the only engine Arc")
            .protocol
            .as_mut()
            .expect("protocol")
            .roles
            .get_mut("user")
            .expect("user protocol surface")
            .surface
            .trusted_presets = vec![
            ClientTrustedPresetDescriptor {
                name: "x-default-status".into(),
                codec: "string".into(),
            },
            ClientTrustedPresetDescriptor {
                name: "x-order-id".into(),
                codec: "string".into(),
            },
        ];
        engine
    }

    #[cfg(feature = "sqlite")]
    fn distributed_extension(response: &Response) -> serde_json::Value {
        serde_json::to_value(
            response
                .extensions
                .get("distributed")
                .expect("configured protocol response must carry one envelope"),
        )
        .unwrap()
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn client_manifest_exports_the_exact_runtime_execution_limits() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect_lazy("sqlite::memory:")
            .unwrap();
        let project = DistributedProjectManifest::new("orders-service").table_schema(orders());
        let engine = GraphqlEngine::from_manifest(&project, pool)
            .unwrap()
            .roles(&["user"])
            .grant_all("user")
            .max_depth(6)
            .max_complexity(37)
            .build()
            .unwrap();

        let manifest = engine.client_manifest_for_role("user").unwrap();
        assert_eq!(manifest.execution.max_depth, 6);
        assert_eq!(manifest.execution.max_complexity, 37);
        assert_eq!(manifest.execution.complexity.version, 1);
        assert_eq!(manifest.execution.complexity.scalar, 1);
        assert_eq!(manifest.execution.complexity.list_fanout, 5);
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn graphiql_isolated_introspection_does_not_change_the_client_contract() {
        let build = |graphiql| {
            let pool = sqlx::sqlite::SqlitePoolOptions::new()
                .connect_lazy("sqlite::memory:")
                .unwrap();
            let project = DistributedProjectManifest::new("orders-service").table_schema(orders());
            GraphqlEngine::from_manifest(&project, pool)
                .unwrap()
                .roles(&["user"])
                .grant_all("user")
                .client_application_surface("console", ["user"])
                .protocol_token_key([7; 32])
                .graphiql(graphiql)
                .build()
                .unwrap()
        };
        let without_graphiql = build(false);
        let with_graphiql = build(true);
        let generated = without_graphiql
            .client_manifest_for_application("console", &["user"])
            .unwrap();
        let runtime = with_graphiql
            .client_manifest_for_application("console", &["user"])
            .unwrap();

        assert_eq!(generated, runtime);
        assert_eq!(runtime.execution.max_depth, 8);
        assert_eq!(runtime.execution.max_complexity, 500);
        assert_eq!(
            with_graphiql.inner.schemas.get("user").unwrap().sdl(),
            with_graphiql
                .inner
                .graphiql_schemas
                .get("user")
                .unwrap()
                .sdl()
        );

        let mut session = Session::new();
        session.set("x-role", "user");
        let deep_introspection = r#"
            query GraphiqlIntrospection {
              __type(name: "OrderView") {
                fields {
                  type {
                    ofType {
                      ofType {
                        ofType {
                          ofType {
                            ofType {
                              ofType {
                                ofType {
                                  name
                                }
                              }
                            }
                          }
                        }
                      }
                    }
                  }
                }
              }
            }
        "#;
        let strict = without_graphiql
            .execute(&session, Request::new(deep_introspection))
            .await;
        assert!(
            strict.is_err(),
            "the normal schema must retain the manifest-fingerprinted depth limit"
        );
        let relaxed = with_graphiql
            .execute(&session, Request::new(deep_introspection))
            .await;
        assert!(
            !relaxed.is_err(),
            "pure GraphiQL introspection should use its isolated allowance: {:?}",
            relaxed.errors
        );

        let mut dag_request = Request::new(duplicated_introspection_fragment_dag(32));
        assert!(
            !has_multiple_protocol_query_roots(&with_graphiql.inner, "user", &mut dag_request),
            "protocol root analysis must memoize shared fragment DAGs"
        );
        let executable_dag = duplicated_introspection_fragment_dag(12);
        let dag_response = with_graphiql
            .execute(&session, Request::new(&executable_dag))
            .await;
        assert!(
            !dag_response.is_err(),
            "memoized introspection DAG should execute: {:?}",
            dag_response.errors
        );
        let dag_stream = with_graphiql
            .execute_stream(&session, Request::new(executable_dag))
            .collect::<Vec<_>>()
            .await;
        assert_eq!(dag_stream.len(), 1);
        assert!(
            !dag_stream[0].is_err(),
            "memoized introspection DAG should execute through the streaming path"
        );

        let disabled = with_graphiql
            .execute(
                &session,
                Request::new("{ __schema { queryType { name } } }").disable_introspection(),
            )
            .await;
        assert_eq!(
            disabled.data,
            Value::Null,
            "GraphiQL must not override request-level introspection denial"
        );
        let streamed = with_graphiql
            .execute_stream(
                &session,
                Request::new("{ __schema { queryType { name } } }").disable_introspection(),
            )
            .collect::<Vec<_>>()
            .await;
        assert_eq!(streamed.len(), 1);
        assert_eq!(
            streamed[0].data,
            Value::Null,
            "the streaming path must preserve request-level introspection denial"
        );
    }

    #[test]
    fn graphiql_relaxation_is_selected_only_for_pure_introspection() {
        let classify = |mut request| is_pure_introspection_request(&mut request);
        assert!(classify(Request::new(
            "query Introspection { __schema { queryType { name } } }"
        )));
        assert!(classify(
            Request::new(
                "query App { orders { order_id } } query Introspection { __type(name: \"OrderView\") { name } }"
            )
            .operation_name("Introspection")
        ));
        assert!(classify(Request::new(
            "query ThroughFragment { ...Introspection } fragment Introspection on Query { __schema { queryType { name } } }"
        )));
        assert!(!classify(Request::new(
            "query Mixed { __typename orders { order_id } }"
        )));
        assert!(!classify(Request::new(
            "query App { orders { order_id } } query Introspection { __schema { queryType { name } } }"
        )));
        assert!(!classify(Request::new(
            "mutation NotIntrospection { __typename }"
        )));
        assert!(!classify(Request::new(
            "query ThroughFragment { ...Missing }"
        )));
        assert!(!classify(
            Request::new("{ __schema { queryType { name } } }").disable_introspection()
        ));

        let mut cached_application =
            Request::new("query Introspection { __schema { queryType { name } } }");
        cached_application.set_parsed_query(
            async_graphql::parser::parse_query("query App { orders { order_id } }").unwrap(),
        );
        assert!(
            !is_pure_introspection_request(&mut cached_application),
            "classification must inspect the same cached AST async-graphql executes"
        );

        assert!(
            classify(Request::new(duplicated_introspection_fragment_dag(32))),
            "shared fragment DAGs must be memoized rather than expanded exponentially"
        );

        let over_budget = (0..=REQUEST_ANALYSIS_MAX_SELECTIONS)
            .map(|index| format!("f{index}: __typename"))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            !classify(Request::new(format!("query TooWide {{ {over_budget} }}"))),
            "untrusted classifier work must remain explicitly bounded"
        );
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn configured_protocol_attaches_stable_role_and_identity_bound_envelopes() {
        use crate::graphql::identity::VerifiedPrincipal;

        let engine = protocol_engine("public/graphql");
        let manifest = engine.client_manifest_for_role("user").unwrap();
        let role_info = &engine
            .inner
            .protocol
            .as_ref()
            .unwrap()
            .roles
            .get("user")
            .unwrap();
        assert_eq!(
            role_info.surface.schema_fingerprint,
            manifest.schema_fingerprint
        );
        assert_eq!(
            role_info.surface.protocol_fingerprint,
            manifest.protocol_fingerprint
        );

        let mut session = Session::new();
        session.set("x-role", "user");
        session.set("x-tenant", "tenant-a");
        let principal = VerifiedPrincipal::test_oidc(
            "https://issuer.example",
            "principal-a",
            &["orders-service"],
        );
        let first = engine
            .execute(
                &session,
                Request::new("{ __typename }").data(principal.clone()),
            )
            .await;
        let second = engine
            .execute(
                &session,
                Request::new("{ __typename }").data(principal.clone()),
            )
            .await;
        let first = distributed_extension(&first);
        let second = distributed_extension(&second);
        assert_eq!(first, second);
        assert_eq!(first["protocolVersion"], 2);
        assert_eq!(first["schemaHash"], manifest.schema_fingerprint);
        assert_eq!(
            first["operation"],
            "sha256:7f56e67dd21ab3f30d1ff8b7bed08893f0a0db86449836189b361dd1e56ddb4b"
        );
        let scope = first["cacheScope"].as_str().unwrap();
        assert!(scope.starts_with("v1.cache-scope."));
        assert!(!scope.contains("principal-a"));
        assert!(!scope.contains("tenant-a"));

        let generated_role_request = |name: &str, schema_hash: &str| -> Request {
            serde_json::from_value(serde_json::json!({
                "query": "{ __typename }",
                "extensions": {
                    "distributed": {
                        "client": {
                            "surface": {"kind": "role", "name": name},
                            "schemaHash": schema_hash
                        }
                    }
                }
            }))
            .expect("generated role request")
        };
        let generated_response = engine
            .execute(
                &session,
                generated_role_request("user", &manifest.schema_fingerprint)
                    .data(principal.clone()),
            )
            .await;
        assert_eq!(first, distributed_extension(&generated_response));
        for invalid in [
            generated_role_request("admin", &manifest.schema_fingerprint),
            generated_role_request("user", "sha256:stale-generation"),
        ] {
            let response = engine.execute(&session, invalid).await;
            assert!(response.is_err());
            assert!(!response.extensions.contains_key("distributed"));
        }

        let mut other_session = session.clone();
        other_session.set("user-agent", "a totally different browser");
        let other_session_response = engine
            .execute(
                &other_session,
                Request::new("{ __typename }").data(principal.clone()),
            )
            .await;
        assert_eq!(
            first["cacheScope"],
            distributed_extension(&other_session_response)["cacheScope"]
        );

        let mut other_user = session.clone();
        other_user.set("x-user-id", "user-b");
        let other_user_response = engine
            .execute(
                &other_user,
                Request::new("{ __typename }").data(principal.clone()),
            )
            .await;
        assert_ne!(
            first["cacheScope"],
            distributed_extension(&other_user_response)["cacheScope"]
        );

        let other_principal = VerifiedPrincipal::test_oidc(
            "https://issuer.example",
            "principal-b",
            &["orders-service"],
        );
        let other_principal_response = engine
            .execute(
                &session,
                Request::new("{ __typename }").data(other_principal),
            )
            .await;
        assert_ne!(
            first["cacheScope"],
            distributed_extension(&other_principal_response)["cacheScope"]
        );

        let other_namespace = protocol_engine("internal/graphql");
        let namespaced_response = other_namespace
            .execute(&session, Request::new("{ __typename }").data(principal))
            .await;
        assert_ne!(
            first["cacheScope"],
            distributed_extension(&namespaced_response)["cacheScope"]
        );
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn named_application_protocol_selection_is_registered_exact_and_role_bound() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect_lazy("sqlite::memory:")
            .unwrap();
        let project = DistributedProjectManifest::new("orders-service").table_schema(orders());
        let engine = GraphqlEngine::from_manifest(&project, pool)
            .unwrap()
            .roles(&["admin", "user"])
            .grant_all("admin")
            .grant_all("user")
            .client_application_surface("console", ["admin", "user"])
            .protocol_token_key([7; 32])
            .build()
            .unwrap();
        let manifest = engine
            .client_manifest_for_application("console", &["user", "admin"])
            .expect("registered application manifest");
        assert!(engine
            .client_manifest_for_application("console", &["user"])
            .is_err());

        let request = |schema_hash: &str, roles: serde_json::Value| -> Request {
            serde_json::from_value(serde_json::json!({
                "query": "{ __typename }",
                "extensions": {
                    "distributed": {
                        "client": {
                            "surface": {
                                "kind": "application",
                                "name": "console",
                                "roles": roles
                            },
                            "schemaHash": schema_hash
                        }
                    }
                }
            }))
            .expect("generated application request")
        };

        let mut user = Session::new();
        user.set("x-role", "user");
        user.set("x-user-id", "person-1");
        let user_response = engine
            .execute(
                &user,
                request(
                    &manifest.schema_fingerprint,
                    serde_json::json!(["admin", "user"]),
                ),
            )
            .await;
        let user_envelope = distributed_extension(&user_response);
        assert_eq!(user_envelope["schemaHash"], manifest.schema_fingerprint);

        let mut admin = user.clone();
        admin.set("x-role", "admin");
        let admin_response = engine
            .execute(
                &admin,
                request(
                    &manifest.schema_fingerprint,
                    serde_json::json!(["admin", "user"]),
                ),
            )
            .await;
        let admin_envelope = distributed_extension(&admin_response);
        assert_eq!(admin_envelope["schemaHash"], manifest.schema_fingerprint);
        assert_ne!(
            user_envelope["cacheScope"], admin_envelope["cacheScope"],
            "one application schema never erases the concrete authorized role"
        );

        for invalid in [
            request(
                &manifest.schema_fingerprint,
                serde_json::json!(["user", "admin"]),
            ),
            request(
                "sha256:stale-generation",
                serde_json::json!(["admin", "user"]),
            ),
        ] {
            let response = engine.execute(&user, invalid).await;
            assert!(response.is_err());
            assert!(!response.extensions.contains_key("distributed"));
        }
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn trusted_presets_are_session_derived_typed_and_scope_bound() {
        let engine = preset_protocol_engine();
        assert_eq!(
            engine.inner.protocol.as_ref().unwrap().roles["user"]
                .surface
                .trusted_presets,
            vec![
                ClientTrustedPresetDescriptor {
                    name: "x-default-status".into(),
                    codec: "string".into(),
                },
                ClientTrustedPresetDescriptor {
                    name: "x-order-id".into(),
                    codec: "string".into(),
                },
            ]
        );

        let mut session = Session::new();
        session.set("x-role", "user");
        session.set("x-user-id", "person-1");
        session.set("x-order-id", "order-1");
        session.set("x-default-status", "assigned");
        let first = engine
            .execute(&session, Request::new("{ __typename }"))
            .await;
        let first = distributed_extension(&first);
        assert_eq!(
            first["trustedPresets"],
            serde_json::json!([
                {"name": "x-default-status", "codec": "string", "value": "assigned"},
                {"name": "x-order-id", "codec": "string", "value": "order-1"}
            ])
        );

        let mut changed = session.clone();
        changed.set("x-default-status", "queued");
        let changed = engine
            .execute(&changed, Request::new("{ __typename }"))
            .await;
        let changed = distributed_extension(&changed);
        assert_ne!(first["cacheScope"], changed["cacheScope"]);
        assert_eq!(changed["trustedPresets"][0]["value"], "queued");

        let mut missing = Session::new();
        missing.set("x-role", "user");
        missing.set("x-user-id", "person-1");
        missing.set("x-order-id", "order-1");
        let response = engine
            .execute(&missing, Request::new("{ __typename }"))
            .await;
        assert!(response.is_err());
        assert!(!response.extensions.contains_key("distributed"));
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn row_policy_presets_follow_sql_claim_case_normalization() {
        let engine = policy_protocol_engine("mixed-case-policy", "X-Tenant");
        assert_eq!(
            engine.inner.protocol.as_ref().unwrap().roles["user"]
                .surface
                .trusted_presets,
            vec![ClientTrustedPresetDescriptor {
                name: "X-Tenant".into(),
                codec: "string".into(),
            }]
        );

        let mut session = Session::new();
        session.set("x-role", "user");
        session.set("x-user-id", "person-1");
        // `operand_to_bind` accepts the normalized lowercase header for this
        // mixed-case policy claim. The cache-scope envelope must expose the
        // same resolved value or client policy evaluation would fail closed.
        session.set("x-tenant", "tenant-1");
        let response = engine
            .execute(&session, Request::new("{ __typename }"))
            .await;
        assert_eq!(
            distributed_extension(&response)["trustedPresets"],
            serde_json::json!([
                {"name": "X-Tenant", "codec": "string", "value": "tenant-1"}
            ])
        );
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn cache_scope_tracks_only_relevant_claims_and_private_policy() {
        use crate::graphql::identity::VerifiedPrincipal;

        let engine = policy_protocol_engine("public/graphql", "x-tenant");
        let principal = VerifiedPrincipal::test_oidc(
            "https://issuer.example",
            "principal-a",
            &["orders-service"],
        );
        let mut session = Session::new();
        session.set("x-role", "user");
        session.set("x-user-id", "user-a");
        session.set("x-tenant", "tenant-a");
        session.set("x-organization", "organization-a");
        let response = engine
            .execute(
                &session,
                Request::new("{ __typename }").data(principal.clone()),
            )
            .await;
        let envelope = distributed_extension(&response);

        let mut irrelevant = session.clone();
        irrelevant.set("cookie", "rotated-cookie");
        irrelevant.set("x-organization", "organization-b");
        let response = engine
            .execute(
                &irrelevant,
                Request::new("{ __typename }").data(principal.clone()),
            )
            .await;
        assert_eq!(
            envelope["cacheScope"],
            distributed_extension(&response)["cacheScope"]
        );

        let mut other_tenant = session.clone();
        other_tenant.set("x-tenant", "tenant-b");
        let response = engine
            .execute(
                &other_tenant,
                Request::new("{ __typename }").data(principal.clone()),
            )
            .await;
        assert_ne!(
            envelope["cacheScope"],
            distributed_extension(&response)["cacheScope"]
        );

        let other_policy = policy_protocol_engine("public/graphql", "x-organization");
        let response = other_policy
            .execute(
                &session,
                Request::new("{ __typename }").data(principal.clone()),
            )
            .await;
        assert_ne!(
            envelope["cacheScope"],
            distributed_extension(&response)["cacheScope"]
        );

        let mut anonymous = session;
        anonymous.set("x-role", "anonymous");
        let response = engine
            .execute(&anonymous, Request::new("{ __typename }").data(principal))
            .await;
        assert_ne!(
            envelope["cacheScope"],
            distributed_extension(&response)["cacheScope"]
        );
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn protocol_stream_uses_one_request_accumulator_and_raw_engine_has_no_envelope() {
        use crate::graphql::identity::VerifiedPrincipal;

        let engine = protocol_engine("public/graphql");
        let mut session = Session::new();
        session.set("x-role", "user");
        let principal = VerifiedPrincipal::test_oidc(
            "https://issuer.example",
            "principal-a",
            &["orders-service"],
        );
        let mut responses =
            engine.execute_stream(&session, Request::new("{ __typename }").data(principal));
        let response = responses.next().await.expect("one query response");
        assert!(response.extensions.contains_key("distributed"));
        assert!(responses.next().await.is_none());

        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect_lazy("sqlite::memory:")
            .unwrap();
        let project = DistributedProjectManifest::new("orders-service").table_schema(orders());
        let raw = GraphqlEngine::from_manifest(&project, pool)
            .unwrap()
            .roles(&["user"])
            .grant_all("user")
            .build()
            .unwrap();
        let response = raw.execute(&session, Request::new("{ __typename }")).await;
        assert!(!response.extensions.contains_key("distributed"));
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn protocol_configuration_requires_real_key_and_service_identity() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect_lazy("sqlite::memory:")
            .unwrap();
        let result = GraphqlEngine::builder(pool.clone())
            .service_id("orders-service")
            .protocol_token_key([0; 32])
            .build();
        let error = match result {
            Ok(_) => panic!("all-zero protocol key must fail"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("must not be all zero"));

        let result = GraphqlEngine::builder(pool)
            .protocol_token_key([7; 32])
            .build();
        let error = match result {
            Ok(_) => panic!("protocol key without service identity must fail"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("stable service ID"));
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn one_role_surface_drives_runtime_sdl_manifest_and_limits() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect_lazy("sqlite::memory:")
            .unwrap();
        let project = DistributedProjectManifest::new("orders-service").table_schema(orders());
        let commands = TypedCommandInventory::from_contracts(&[test_command::<
            ChangeOrderInput,
            ChangeOrderPayload,
        >(
            "order.refresh",
            "orders_refresh",
            &["user"],
        )])
        .unwrap();
        let mut builder = GraphqlEngine::from_manifest(&project, pool)
            .unwrap()
            .roles(&["user"])
            .grant_all("user")
            .default_limit(7)
            .max_limit(19)
            .client_projectors([SurfaceProjector::new("project_orders")
                .facts(["order.changed"])
                .models(["OrderView"])]);
        builder.command_binding = Some(test_service_binding("orders-service", &commands));
        builder.typed_commands = commands;
        let engine = builder.build().unwrap();

        let stored = engine.surface_for_role("user").unwrap();
        assert_eq!(engine.service_id(), Some("orders-service"));
        let export = engine.client_surface_for_role("user").unwrap();
        assert!(Arc::ptr_eq(&stored, export.surface()));
        let manifest = export.manifest().unwrap();
        assert_eq!(manifest, export.manifest().unwrap());
        assert_eq!(manifest.service_id, "orders-service");
        let static_sdl = engine.ir_sdl_for_role("user").unwrap();
        let runtime_sdl = engine.sdl_for_role("user").unwrap();

        for type_name in [
            "Query",
            "Subscription",
            "Mutation",
            "OrderView",
            "orders_aggregate",
            "orders_aggregate_fields",
        ] {
            assert_eq!(
                type_field_names(&static_sdl, type_name),
                type_field_names(&runtime_sdl, type_name),
                "runtime/static field drift for {type_name}"
            );
        }

        let query_roots: BTreeSet<String> = manifest
            .roots
            .iter()
            .filter(|root| root.operation == ClientRootOperation::Query)
            .map(|root| root.name.clone())
            .collect();
        let subscription_roots: BTreeSet<String> = manifest
            .roots
            .iter()
            .filter(|root| root.operation == ClientRootOperation::Subscription)
            .map(|root| root.name.clone())
            .collect();
        let runtime_query_roots = type_field_names(&runtime_sdl, "Query")
            .into_iter()
            .filter(|field| field != "commandStatus")
            .collect();
        assert_eq!(query_roots, runtime_query_roots);
        assert_eq!(
            subscription_roots,
            type_field_names(&runtime_sdl, "Subscription")
        );
        assert_eq!(manifest.commands.len(), 1);
        assert_eq!(manifest.commands[0].name, "order.refresh");
        assert_eq!(
            manifest
                .protocol_operations
                .command_status
                .as_ref()
                .unwrap()
                .name,
            "Distributed_CommandStatus"
        );
        assert_eq!(
            type_field_names(&runtime_sdl, "Mutation"),
            BTreeSet::from(["orders_refresh".into()])
        );
        assert_eq!(
            manifest.models[0]
                .fields
                .iter()
                .map(|field| field.name.clone())
                .collect::<BTreeSet<_>>(),
            type_field_names(&runtime_sdl, "OrderView")
        );
        assert_eq!(subscription_roots, BTreeSet::from(["orders".into()]));
        assert!(!runtime_sdl.contains("type Subscription {\n\torders_by_pk"));

        let list = manifest
            .roots
            .iter()
            .find(|root| root.id == "query:orders")
            .unwrap();
        assert_eq!(list.pagination.as_ref().unwrap().default_limit, 7);
        assert_eq!(list.pagination.as_ref().unwrap().max_limit, 19);
        assert_eq!(manifest.projectors[0].name, "project_orders");
        let runtime_json = input_field_names(&runtime_sdl, "JSON_comparison_exp");
        assert_eq!(
            runtime_json,
            input_field_names(&static_sdl, "JSON_comparison_exp")
        );
        let metadata_ops: BTreeSet<String> = list
            .filter
            .as_ref()
            .unwrap()
            .fields
            .iter()
            .find(|field| field.name == "metadata")
            .unwrap()
            .operators
            .iter()
            .cloned()
            .collect();
        assert_eq!(metadata_ops, runtime_json);
        for forbidden in ["_contains", "_contained_in", "_has_key"] {
            assert!(!metadata_ops.contains(forbidden));
        }
        assert_eq!(
            manifest.schema_fingerprint,
            "sha256:ab2e533efd19ce48b480deb8fd80895f631f43dd72d3d9b38823df8eb738110b"
        );
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn manual_engine_client_export_requires_explicit_service_id() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect_lazy("sqlite::memory:")
            .unwrap();
        let engine = GraphqlEngine::builder(pool)
            .register_schema_exposed(orders())
            .unwrap()
            .roles(&["user"])
            .grant_all("user")
            .build()
            .unwrap();

        let error = engine.client_surface_for_role("user").unwrap_err();
        assert!(error
            .to_string()
            .contains("GraphqlEngineBuilder::service_id"));
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn empty_role_static_runtime_and_manifest_are_truthful() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect_lazy("sqlite::memory:")
            .unwrap();
        let project = DistributedProjectManifest::new("orders-service").table_schema(orders());
        let engine = GraphqlEngine::from_manifest(&project, pool)
            .unwrap()
            .roles(&["empty"])
            .build()
            .unwrap();

        let static_sdl = engine.ir_sdl_for_role("empty").unwrap();
        let runtime_sdl = engine.sdl_for_role("empty").unwrap();
        assert_eq!(
            type_field_names(&static_sdl, "Query"),
            BTreeSet::from(["_empty".into()])
        );
        assert_eq!(
            type_field_names(&static_sdl, "Query"),
            type_field_names(&runtime_sdl, "Query")
        );
        let manifest = engine.client_manifest_for_role("empty").unwrap();
        assert!(manifest.roots.is_empty());
        assert!(!manifest.capabilities.live_queries);
    }

    #[cfg(feature = "postgres")]
    #[tokio::test]
    async fn postgres_role_surface_drives_runtime_sdl_and_manifest() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@localhost/distributed_test")
            .unwrap();
        let project = DistributedProjectManifest::new("orders-service").table_schema(orders());
        let engine = GraphqlEngine::from_manifest(&project, pool)
            .unwrap()
            .roles(&["user"])
            .grant_all("user")
            .default_limit(7)
            .max_limit(19)
            .build()
            .unwrap();

        let export = engine.client_surface_for_role("user").unwrap();
        let manifest = export.manifest().unwrap();
        assert_eq!(manifest, export.manifest().unwrap());
        let static_sdl = engine.ir_sdl_for_role("user").unwrap();
        let runtime_sdl = engine.sdl_for_role("user").unwrap();
        for type_name in [
            "Query",
            "Subscription",
            "OrderView",
            "orders_aggregate",
            "orders_aggregate_fields",
        ] {
            assert_eq!(
                type_field_names(&static_sdl, type_name),
                type_field_names(&runtime_sdl, type_name),
                "Postgres runtime/static field drift for {type_name}"
            );
        }

        let query_roots: BTreeSet<String> = manifest
            .roots
            .iter()
            .filter(|root| root.operation == ClientRootOperation::Query)
            .map(|root| root.name.clone())
            .collect();
        let runtime_query_roots = type_field_names(&runtime_sdl, "Query")
            .into_iter()
            .filter(|field| field != "commandStatus")
            .collect();
        assert_eq!(query_roots, runtime_query_roots);
        assert_eq!(
            manifest.models[0]
                .fields
                .iter()
                .map(|field| field.name.clone())
                .collect::<BTreeSet<_>>(),
            type_field_names(&runtime_sdl, "OrderView")
        );

        let runtime_json = input_field_names(&runtime_sdl, "JSON_comparison_exp");
        assert_eq!(
            runtime_json,
            input_field_names(&static_sdl, "JSON_comparison_exp")
        );
        let metadata_ops: BTreeSet<String> = manifest
            .roots
            .iter()
            .find(|root| root.id == "query:orders")
            .unwrap()
            .filter
            .as_ref()
            .unwrap()
            .fields
            .iter()
            .find(|field| field.name == "metadata")
            .unwrap()
            .operators
            .iter()
            .cloned()
            .collect();
        assert_eq!(metadata_ops, runtime_json);
        for required in ["_contains", "_contained_in", "_has_key"] {
            assert!(metadata_ops.contains(required));
        }
        assert_eq!(manifest.service_id, "orders-service");
        assert_eq!(
            manifest.schema_fingerprint,
            "sha256:9b2118ae9fc68ebdcaf0029452bb5e3f3448a6774963c99f2fb20718817a608a"
        );
    }

    fn customers() -> TableSchema {
        TableSchema {
            model_name: "CustomerView".into(),
            table_name: "customers".into(),
            columns: vec![
                TableColumn {
                    primary_key: true,
                    ..TableColumn::new("customer_id", "customer_id", ColumnType::Text)
                },
                TableColumn::new("display_name", "display_name", ColumnType::Text),
                TableColumn::new("internal_note", "internal_note", ColumnType::Text),
            ],
            primary_key: PrimaryKey::new(["customer_id"]),
            version_column: None,
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships: Vec::new(),
            kind: TableKind::ReadModel,
        }
    }

    fn type_field(
        name: &str,
        type_name: &str,
        nullable: bool,
        list: bool,
        nested: Option<GraphqlTypeDef>,
    ) -> GraphqlTypeField {
        GraphqlTypeField {
            name: name.into(),
            type_name: type_name.into(),
            nullable,
            list,
            item_nullable: false,
            nested: nested.map(Box::new),
        }
    }

    struct ChangeOrderInput;

    impl GraphqlInputType for ChangeOrderInput {
        fn graphql_type() -> GraphqlTypeDef {
            let patch = GraphqlTypeDef::new(
                "OrderPatchInput",
                vec![
                    type_field("status", "String", false, false, None),
                    type_field("metadata", "JSON", true, false, None),
                ],
            );
            GraphqlTypeDef::new(
                "ChangeOrderInput",
                vec![
                    type_field("patch", "OrderPatchInput", false, false, Some(patch)),
                    type_field("order_id", "String", false, false, None),
                ],
            )
        }
    }

    struct ChangeOrderPayload;

    impl GraphqlOutputType for ChangeOrderPayload {
        fn graphql_type() -> GraphqlTypeDef {
            let changed_order = GraphqlTypeDef::new(
                "ChangedOrder",
                vec![
                    type_field("status", "String", false, false, None),
                    type_field("order_id", "String", false, false, None),
                ],
            );
            GraphqlTypeDef::new(
                "ChangeOrderPayload",
                vec![
                    type_field("warnings", "String", true, true, None),
                    type_field("order", "ChangedOrder", false, false, Some(changed_order)),
                    type_field("accepted", "Boolean", false, false, None),
                ],
            )
        }
    }

    fn matrix_project() -> DistributedProjectManifest {
        DistributedProjectManifest::new("acceptance-service")
            .table_schema(orders())
            .table_schema(customers())
    }

    fn matrix_commands() -> TypedCommandInventory {
        TypedCommandInventory::from_contracts(&[
            test_command::<ChangeOrderInput, ChangeOrderPayload>(
                "order.change",
                "orders_change",
                &["restricted", "admin"],
            ),
            test_command::<ChangeOrderInput, ChangeOrderPayload>(
                "order.force_archive",
                "orders_force_archive",
                &["admin"],
            ),
        ])
        .unwrap()
    }

    fn matrix_projectors() -> Vec<SurfaceProjector> {
        vec![
            SurfaceProjector::new("project_customers")
                .facts(["customer.changed"])
                .models(["CustomerView"]),
            SurfaceProjector::new("project_orders")
                .facts(["order.changed"])
                .models(["OrderView"]),
        ]
    }

    fn restricted_read() -> ReadPermission {
        read()
            .columns(["order_id", "status"])
            .rows(col("status").eq("OPEN"))
            .limit(5)
    }

    fn insert_permission(
        builder: &mut GraphqlEngineBuilder,
        model: &str,
        role: &str,
        permission: ReadPermission,
    ) {
        assert!(builder
            .permissions
            .insert((model.into(), role.into()), RoleModelPerm { permission },)
            .is_none());
    }

    fn matrix_engine(pool: GraphqlPool) -> GraphqlEngine {
        let project = matrix_project();
        let mut builder = GraphqlEngine::from_manifest(&project, pool)
            .unwrap()
            .roles(&["restricted", "admin"])
            .default_limit(11)
            .max_limit(23)
            .client_projectors(matrix_projectors());
        let commands = matrix_commands();
        builder.command_binding = Some(test_service_binding("acceptance-service", &commands));
        builder.typed_commands = commands;
        insert_permission(&mut builder, "OrderView", "restricted", restricted_read());
        insert_permission(
            &mut builder,
            "OrderView",
            "admin",
            read().all_columns().aggregations(),
        );
        insert_permission(
            &mut builder,
            "CustomerView",
            "admin",
            read().all_columns().aggregations(),
        );
        builder.build().unwrap()
    }

    fn independent_manifest(dialect: SurfaceDialect, role: &str) -> DistributedClientManifest {
        let project = matrix_project();
        let options = SurfaceOptions {
            dialect,
            aggregates: true,
            subscriptions: true,
            default_limit: 11,
            max_limit: 23,
        };
        let commands = matrix_commands();
        let full = build_surface(&project.tables, &options)
            .unwrap()
            .with_typed_commands(&commands)
            .unwrap()
            .with_service_binding(Some(test_service_binding("acceptance-service", &commands)))
            .with_projectors(matrix_projectors())
            .unwrap();
        let grants = match role {
            "restricted" => BTreeMap::from([(
                "OrderView".into(),
                RoleGrant::columns(["order_id", "status"])
                    .rows(col("status").eq("OPEN"))
                    .limit(5),
            )]),
            "admin" => BTreeMap::from([
                (
                    "OrderView".into(),
                    RoleGrant::all_columns().with_aggregations(),
                ),
                (
                    "CustomerView".into(),
                    RoleGrant::all_columns().with_aggregations(),
                ),
            ]),
            other => panic!("unexpected matrix role `{other}`"),
        };
        let selected = surface_for_role(&full, role, &grants).unwrap();
        DistributedClientSurfaceExport::from_project(&project, selected)
            .unwrap()
            .manifest()
            .unwrap()
    }

    fn definition_inventory(sdl: &str) -> BTreeMap<String, BTreeSet<String>> {
        let mut inventory: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut current: Option<String> = None;
        for line in sdl.lines() {
            let line = line.trim();
            if current.is_none() {
                let declaration = line
                    .strip_prefix("type ")
                    .or_else(|| line.strip_prefix("input "));
                if let Some(declaration) = declaration {
                    if line.contains('{') {
                        let name = declaration
                            .split([' ', '{'])
                            .next()
                            .expect("definition name")
                            .to_string();
                        inventory.entry(name.clone()).or_default();
                        current = Some(name);
                    }
                }
                continue;
            }
            if line == "}" {
                current = None;
                continue;
            }
            if line.is_empty() || line.starts_with('#') || line.starts_with('"') {
                continue;
            }
            let field = line
                .split(['(', ':'])
                .next()
                .map(str::trim)
                .filter(|field| !field.is_empty());
            if let (Some(definition), Some(field)) = (&current, field) {
                inventory
                    .get_mut(definition)
                    .expect("current definition")
                    .insert(field.into());
            }
        }
        inventory
    }

    fn sha256(bytes: &[u8]) -> String {
        format!("sha256:{:x}", Sha256::digest(bytes))
    }

    #[derive(Clone, Copy)]
    struct ArtifactGoldens {
        manifest: &'static str,
        static_sdl: &'static str,
        runtime_sdl: &'static str,
    }

    async fn assert_role_matrix(
        engine: &GraphqlEngine,
        dialect: SurfaceDialect,
        role: &str,
        expected: ArtifactGoldens,
    ) {
        let stored = engine.surface_for_role(role).unwrap();
        let export = engine.client_surface_for_role(role).unwrap();
        assert!(Arc::ptr_eq(&stored, export.surface()));
        let manifest = export.manifest().unwrap();
        assert_eq!(manifest, export.manifest().unwrap());
        assert_eq!(manifest, independent_manifest(dialect, role));

        let static_sdl = engine.ir_sdl_for_role(role).unwrap();
        let runtime_sdl = engine.sdl_for_role(role).unwrap();
        assert_eq!(
            definition_inventory(&static_sdl),
            definition_inventory(&runtime_sdl),
            "runtime/static definition drift for role `{role}`"
        );

        let query_roots: BTreeSet<String> = manifest
            .roots
            .iter()
            .filter(|root| root.operation == ClientRootOperation::Query)
            .map(|root| root.name.clone())
            .collect();
        let subscription_roots: BTreeSet<String> = manifest
            .roots
            .iter()
            .filter(|root| root.operation == ClientRootOperation::Subscription)
            .map(|root| root.name.clone())
            .collect();
        let runtime_read_roots = type_field_names(&runtime_sdl, "Query")
            .into_iter()
            .filter(|field| field != "commandStatus")
            .collect::<BTreeSet<_>>();
        assert_eq!(query_roots, runtime_read_roots);
        assert_eq!(
            subscription_roots,
            type_field_names(&runtime_sdl, "Subscription")
        );
        let expected_commands = if role == "admin" { 2 } else { 1 };
        assert_eq!(manifest.commands.len(), expected_commands);
        assert_eq!(
            manifest
                .protocol_operations
                .command_status
                .as_ref()
                .unwrap()
                .name,
            "Distributed_CommandStatus"
        );
        for model in &manifest.models {
            let expected_fields: BTreeSet<String> = model
                .fields
                .iter()
                .map(|field| field.name.clone())
                .chain(model.relationships.iter().flat_map(|relationship| {
                    std::iter::once(relationship.name.clone()).chain(
                        relationship
                            .aggregate
                            .iter()
                            .map(|aggregate| aggregate.name.clone()),
                    )
                }))
                .collect();
            assert_eq!(
                expected_fields,
                type_field_names(&static_sdl, &model.typename),
                "manifest/static model drift for {}",
                model.typename
            );
            assert_eq!(
                expected_fields,
                type_field_names(&runtime_sdl, &model.typename),
                "manifest/runtime model drift for {}",
                model.typename
            );
        }

        let model_ids: BTreeSet<_> = manifest
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect();
        let command_names: BTreeSet<_> = manifest
            .commands
            .iter()
            .map(|command| command.name.as_str())
            .collect();
        let projector_names: BTreeSet<_> = manifest
            .projectors
            .iter()
            .map(|projector| projector.name.as_str())
            .collect();
        match role {
            "restricted" => {
                assert_eq!(model_ids, BTreeSet::from(["OrderView"]));
                assert_eq!(command_names, BTreeSet::from(["order.change"]));
                assert_eq!(
                    type_field_names(&runtime_sdl, "Mutation"),
                    BTreeSet::from(["orders_change".into()])
                );
                assert_eq!(projector_names, BTreeSet::from(["project_orders"]));
                assert!(!query_roots.contains("orders_aggregate"));
                assert_eq!(manifest.models[0].fields.len(), 2);
                assert_eq!(
                    manifest
                        .roots
                        .iter()
                        .find(|root| root.id == "query:orders")
                        .unwrap()
                        .pagination
                        .as_ref()
                        .unwrap()
                        .default_limit,
                    5
                );
            }
            "admin" => {
                assert_eq!(model_ids, BTreeSet::from(["CustomerView", "OrderView"]));
                assert_eq!(
                    command_names,
                    BTreeSet::from(["order.change", "order.force_archive"])
                );
                assert_eq!(
                    type_field_names(&runtime_sdl, "Mutation"),
                    BTreeSet::from(["orders_change".into(), "orders_force_archive".into()])
                );
                assert_eq!(
                    projector_names,
                    BTreeSet::from(["project_customers", "project_orders"])
                );
                assert!(query_roots.contains("customers_aggregate"));
                assert!(query_roots.contains("orders_aggregate"));
            }
            other => panic!("unexpected matrix role `{other}`"),
        }

        let manifest_json = serde_json::to_vec(&manifest).unwrap();
        let actual_manifest = sha256(&manifest_json);
        let actual_static_sdl = sha256(static_sdl.as_bytes());
        let actual_runtime_sdl = sha256(runtime_sdl.as_bytes());
        assert_eq!(actual_manifest, expected.manifest, "{dialect:?}/{role}");
        assert_eq!(actual_static_sdl, expected.static_sdl, "{dialect:?}/{role}");
        assert_eq!(
            actual_runtime_sdl, expected.runtime_sdl,
            "{dialect:?}/{role}"
        );
    }

    async fn assert_nested_command_validates(engine: &GraphqlEngine) {
        let operation = "mutation Client_orders_change($commandId: ID!, $input: ChangeOrderInput!) { orders_change(commandId: $commandId, input: $input) { accepted order { order_id status } warnings } }";
        async_graphql::parser::parse_query(operation)
            .expect("generated command operation must parse");

        let request = Request::new(operation).variables(async_graphql::Variables::from_json(
            serde_json::json!({
                "commandId": "0190a000-0000-7000-8000-000000000042",
                "input": {
                    "order_id": "order-1",
                    "patch": {
                        "metadata": {"source": "acceptance"},
                        "status": "READY"
                    }
                }
            }),
        ));
        let mut session = Session::new();
        session.set(crate::microsvc::ROLE_KEY, "admin");
        let response = engine.execute(&session, request).await;
        assert_eq!(response.errors.len(), 1, "{response:?}");
        assert_eq!(
            response.errors[0].message,
            "command dispatcher not configured (use graphql_router_with_service)"
        );
    }

    #[cfg(feature = "sqlite")]
    const SQLITE_RESTRICTED_GOLDENS: ArtifactGoldens = ArtifactGoldens {
        manifest: "sha256:a142da405f0d5b4dd0f388f6158f6b70a0f7ade3f360cecccd14f07412abe331",
        static_sdl: "sha256:c94afb7de76b34c6e36b897643d9523afa8872fa480bf104d8f54f95ef73ea0a",
        runtime_sdl: "sha256:cf35a2fd5309ab6ca893b5820f4a5efdd9eef1df83013dbf6ac0ffdf63710e8e",
    };

    #[cfg(feature = "sqlite")]
    const SQLITE_ADMIN_GOLDENS: ArtifactGoldens = ArtifactGoldens {
        manifest: "sha256:d4cb632b88d7c3779d0cb373858b8dfb5907cb31bbf64ea0f41f2b03f43b8dfa",
        static_sdl: "sha256:8ffed8116b16792ab8f81940999489d73b51615eb863d32b1640446535913b89",
        runtime_sdl: "sha256:d94970d5e6ca8745ec97a286332f7cf1a372e47f5e8e10df10bd67fd779d54e1",
    };

    #[cfg(feature = "postgres")]
    const POSTGRES_RESTRICTED_GOLDENS: ArtifactGoldens = ArtifactGoldens {
        manifest: "sha256:a142da405f0d5b4dd0f388f6158f6b70a0f7ade3f360cecccd14f07412abe331",
        static_sdl: "sha256:c94afb7de76b34c6e36b897643d9523afa8872fa480bf104d8f54f95ef73ea0a",
        runtime_sdl: "sha256:cf35a2fd5309ab6ca893b5820f4a5efdd9eef1df83013dbf6ac0ffdf63710e8e",
    };

    #[cfg(feature = "postgres")]
    const POSTGRES_ADMIN_GOLDENS: ArtifactGoldens = ArtifactGoldens {
        manifest: "sha256:679e700c97004d6aeacb34595a10ddbf1b939b2d3fa8bb175aa63b8a78513016",
        static_sdl: "sha256:5d3416d9926a5374ee95408c38e74bfd03b1b7ef4e2f7ee1f81752a102c4a989",
        runtime_sdl: "sha256:08e621d86f6606260e653c67d27899f9c93a2ac4545ec0741d78445981d3d46f",
    };

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn sqlite_restricted_admin_full_artifact_matrix() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect_lazy("sqlite::memory:")
            .unwrap();
        let engine = matrix_engine(pool.into());
        assert_role_matrix(
            &engine,
            SurfaceDialect::Sqlite,
            "restricted",
            SQLITE_RESTRICTED_GOLDENS,
        )
        .await;
        assert_role_matrix(
            &engine,
            SurfaceDialect::Sqlite,
            "admin",
            SQLITE_ADMIN_GOLDENS,
        )
        .await;
        assert_nested_command_validates(&engine).await;
    }

    #[cfg(feature = "postgres")]
    #[tokio::test]
    async fn postgres_restricted_admin_full_artifact_matrix() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@localhost/distributed_test")
            .unwrap();
        let engine = matrix_engine(pool.into());
        assert_role_matrix(
            &engine,
            SurfaceDialect::Postgres,
            "restricted",
            POSTGRES_RESTRICTED_GOLDENS,
        )
        .await;
        assert_role_matrix(
            &engine,
            SurfaceDialect::Postgres,
            "admin",
            POSTGRES_ADMIN_GOLDENS,
        )
        .await;
        assert_nested_command_validates(&engine).await;
    }

    #[cfg(feature = "sqlite")]
    fn composite_records() -> TableSchema {
        TableSchema {
            model_name: "CompositeRecord".into(),
            table_name: "composite_records".into(),
            columns: vec![
                TableColumn {
                    primary_key: true,
                    ..TableColumn::new("tenant_id", "tenant_id", ColumnType::Text)
                },
                TableColumn {
                    primary_key: true,
                    ..TableColumn::new("record_id", "record_id", ColumnType::Text)
                },
                TableColumn::new("value", "value", ColumnType::Text),
            ],
            primary_key: PrimaryKey::new(["tenant_id", "record_id"]),
            version_column: None,
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships: Vec::new(),
            kind: TableKind::ReadModel,
        }
    }

    #[cfg(feature = "sqlite")]
    fn root_arguments(sdl: &str, field: &str) -> BTreeMap<String, String> {
        let marker = format!("{field}(");
        let arguments = sdl
            .split_once(&marker)
            .unwrap_or_else(|| panic!("missing root field `{field}` in SDL:\n{sdl}"))
            .1
            .split_once(')')
            .expect("root arguments should close")
            .0;
        arguments
            .split(',')
            .filter_map(|argument| argument.trim().split_once(':'))
            .map(|(name, ty)| (name.trim().to_string(), ty.trim().to_string()))
            .collect()
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn isolated_composite_key_root_has_runtime_static_manifest_parity() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE composite_records (\
                tenant_id TEXT NOT NULL, \
                record_id TEXT NOT NULL, \
                value TEXT NOT NULL, \
                PRIMARY KEY (tenant_id, record_id)\
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO composite_records (tenant_id, record_id, value) VALUES \
                ('tenant-a', 'record-1', 'first'), \
                ('tenant-a', 'record-2', 'second')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let project =
            DistributedProjectManifest::new("composite-service").table_schema(composite_records());
        let engine = GraphqlEngine::from_manifest(&project, pool.clone())
            .unwrap()
            .roles(&["admin"])
            .grant_all("admin")
            .build()
            .unwrap();
        let static_sdl = engine.ir_sdl_for_role("admin").unwrap();
        let runtime_sdl = engine.sdl_for_role("admin").unwrap();
        let manifest = engine.client_manifest_for_role("admin").unwrap();
        let by_pk = manifest
            .roots
            .iter()
            .find(|root| root.id == "query:composite_records_by_pk")
            .unwrap();
        assert_eq!(
            by_pk
                .arguments
                .iter()
                .map(|argument| argument.name.as_str())
                .collect::<Vec<_>>(),
            vec!["tenant_id", "record_id"]
        );
        let manifest_arguments: BTreeMap<String, String> = by_pk
            .arguments
            .iter()
            .map(|argument| {
                let mut ty = if argument.list {
                    format!("[{}!]", argument.type_name)
                } else {
                    argument.type_name.clone()
                };
                if !argument.nullable {
                    ty.push('!');
                }
                (argument.name.clone(), ty)
            })
            .collect();
        assert_eq!(
            manifest_arguments,
            BTreeMap::from([
                ("record_id".into(), "String!".into()),
                ("tenant_id".into(), "String!".into()),
            ])
        );
        assert_eq!(
            manifest_arguments,
            root_arguments(&static_sdl, "composite_records_by_pk")
        );
        assert_eq!(
            manifest_arguments,
            root_arguments(&runtime_sdl, "composite_records_by_pk")
        );
        let ModelNormalization::Normalized { fields, encoding } = &manifest.models[0].normalization
        else {
            panic!("isolated composite key must be normalized")
        };
        assert_eq!(
            fields
                .iter()
                .map(|field| field.name.as_str())
                .collect::<Vec<_>>(),
            vec!["tenant_id", "record_id"]
        );
        assert_eq!(encoding, "canonical_json_tuple_v1");

        let mut session = Session::new();
        session.set(crate::microsvc::ROLE_KEY, "admin");
        let response = engine
            .execute(
                &session,
                Request::new(
                    r#"{
                        selected: composite_records_by_pk(
                            tenant_id: "tenant-a"
                            record_id: "record-2"
                        ) {
                            tenant_id
                            record_id
                            value
                        }
                        missing: composite_records_by_pk(
                            tenant_id: "tenant-a"
                            record_id: "record-missing"
                        ) {
                            tenant_id
                            record_id
                            value
                        }
                    }"#,
                ),
            )
            .await;
        assert!(response.errors.is_empty(), "{response:?}");
        assert_eq!(
            response.data.into_json().unwrap(),
            serde_json::json!({
                "selected": {
                    "tenant_id": "tenant-a",
                    "record_id": "record-2",
                    "value": "second"
                },
                "missing": null
            })
        );
    }

    #[cfg(feature = "sqlite")]
    fn simple_records() -> TableSchema {
        TableSchema {
            model_name: "SimpleRecord".into(),
            table_name: "simple_records".into(),
            columns: vec![
                TableColumn {
                    primary_key: true,
                    ..TableColumn::new("simple_id", "simple_id", ColumnType::Text)
                },
                TableColumn::new("tenant_id", "tenant_id", ColumnType::Text),
            ],
            primary_key: PrimaryKey::new(["simple_id"]),
            version_column: None,
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships: Vec::new(),
            kind: TableKind::ReadModel,
        }
    }

    #[cfg(feature = "sqlite")]
    fn policy_parents() -> TableSchema {
        TableSchema {
            model_name: "PolicyParentView".into(),
            table_name: "policy_parents".into(),
            columns: vec![TableColumn {
                primary_key: true,
                ..TableColumn::new("parent_id", "parent_id", ColumnType::Text)
            }],
            primary_key: PrimaryKey::new(["parent_id"]),
            version_column: None,
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships: vec![RelationshipDef {
                field_name: "children".into(),
                kind: RelationshipKind::HasMany,
                target_model: "PolicyChildView".into(),
                foreign_key: Some("parent_id".into()),
                through: None,
                target_foreign_key: None,
            }],
            kind: TableKind::ReadModel,
        }
    }

    #[cfg(feature = "sqlite")]
    fn policy_children() -> TableSchema {
        TableSchema {
            model_name: "PolicyChildView".into(),
            table_name: "policy_children".into(),
            columns: vec![
                TableColumn {
                    primary_key: true,
                    ..TableColumn::new("child_id", "child_id", ColumnType::Text)
                },
                TableColumn::new("parent_id", "parent_id", ColumnType::Text),
                TableColumn::new("label", "label", ColumnType::Text),
                TableColumn::new("visibility", "visibility", ColumnType::Text),
            ],
            primary_key: PrimaryKey::new(["child_id"]),
            version_column: None,
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships: Vec::new(),
            kind: TableKind::ReadModel,
        }
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn relationship_where_applies_the_target_models_row_policy() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("CREATE TABLE policy_parents (parent_id TEXT PRIMARY KEY NOT NULL)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE policy_children (\
                child_id TEXT PRIMARY KEY NOT NULL, \
                parent_id TEXT NOT NULL, \
                label TEXT NOT NULL, \
                visibility TEXT NOT NULL\
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO policy_parents (parent_id) VALUES \
                ('parent-allowed'), \
                ('parent-denied')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO policy_children (child_id, parent_id, label, visibility) VALUES \
                ('child-allowed', 'parent-allowed', 'match', 'allowed'), \
                ('child-denied', 'parent-denied', 'match', 'denied')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let project = DistributedProjectManifest::new("relationship-policy-service")
            .table_schema(policy_parents())
            .table_schema(policy_children());
        let mut builder = GraphqlEngine::from_manifest(&project, pool)
            .unwrap()
            .roles(&["restricted"]);
        insert_permission(
            &mut builder,
            "PolicyParentView",
            "restricted",
            read().all_columns(),
        );
        insert_permission(
            &mut builder,
            "PolicyChildView",
            "restricted",
            read().all_columns().rows(col("visibility").eq("allowed")),
        );
        let engine = builder.build().unwrap();
        let mut session = Session::new();
        session.set(crate::microsvc::ROLE_KEY, "restricted");

        let response = engine
            .execute(
                &session,
                Request::new(
                    r#"{
                        policy_parents(
                            where: { children: { label: { _eq: "match" } } }
                        ) {
                            parent_id
                        }
                    }"#,
                ),
            )
            .await;

        assert!(response.errors.is_empty(), "{response:?}");
        assert_eq!(
            response.data.into_json().unwrap(),
            serde_json::json!({
                "policy_parents": [
                    {"parent_id": "parent-allowed"}
                ]
            })
        );
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn composite_key_relationship_topology_is_rejected_in_both_directions() {
        let cases = [
            {
                let mut composite = composite_records();
                composite.columns.push(TableColumn::new(
                    "simple_id",
                    "simple_id",
                    ColumnType::Text,
                ));
                composite.relationships.push(RelationshipDef {
                    field_name: "simple".into(),
                    kind: RelationshipKind::BelongsTo,
                    target_model: "SimpleRecord".into(),
                    foreign_key: Some("simple_id".into()),
                    through: None,
                    target_foreign_key: None,
                });
                ("outgoing", composite, simple_records())
            },
            {
                let composite = composite_records();
                let mut simple = simple_records();
                simple.relationships.push(RelationshipDef {
                    field_name: "composite".into(),
                    kind: RelationshipKind::BelongsTo,
                    target_model: "CompositeRecord".into(),
                    foreign_key: Some("tenant_id".into()),
                    through: None,
                    target_foreign_key: None,
                });
                ("incoming", composite, simple)
            },
        ];
        for (direction, composite, simple) in cases {
            let pool = sqlx::sqlite::SqlitePoolOptions::new()
                .connect_lazy("sqlite::memory:")
                .unwrap();
            let project = DistributedProjectManifest::new("composite-service")
                .table_schema(composite)
                .table_schema(simple);
            let error = GraphqlEngine::from_manifest(&project, pool)
                .unwrap()
                .roles(&["admin"])
                .grant_all("admin")
                .build()
                .err()
                .expect("composite relationship topology must fail");
            assert!(
                error.to_string().contains("relationship topology"),
                "{direction}: {error}"
            );
        }
    }

    #[cfg(feature = "sqlite")]
    fn metrics() -> TableSchema {
        TableSchema {
            model_name: "MetricView".into(),
            table_name: "metrics".into(),
            columns: vec![
                TableColumn {
                    primary_key: true,
                    ..TableColumn::new("metric_id", "metric_id", ColumnType::Text)
                },
                TableColumn::new("value", "value", ColumnType::Float),
            ],
            primary_key: PrimaryKey::new(["metric_id"]),
            version_column: None,
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships: Vec::new(),
            kind: TableKind::ReadModel,
        }
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn engine_rejects_non_finite_row_policy_literals_and_accepts_finite_values() {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for predicate in [col("value").eq(value), col("value").is_in([value])] {
                let pool = sqlx::sqlite::SqlitePoolOptions::new()
                    .connect_lazy("sqlite::memory:")
                    .unwrap();
                let project =
                    DistributedProjectManifest::new("metrics-service").table_schema(metrics());
                let mut builder = GraphqlEngine::from_manifest(&project, pool)
                    .unwrap()
                    .roles(&["restricted"]);
                insert_permission(
                    &mut builder,
                    "MetricView",
                    "restricted",
                    read().all_columns().rows(predicate),
                );
                let error = builder
                    .build()
                    .err()
                    .expect("non-finite row policy literal must fail");
                assert!(error.to_string().contains("must be finite"), "{error}");
            }
        }

        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect_lazy("sqlite::memory:")
            .unwrap();
        let project = DistributedProjectManifest::new("metrics-service").table_schema(metrics());
        let mut builder = GraphqlEngine::from_manifest(&project, pool)
            .unwrap()
            .roles(&["restricted"]);
        insert_permission(
            &mut builder,
            "MetricView",
            "restricted",
            read().all_columns().rows(FilterExpr::And(vec![
                col("value").eq(1.25_f64),
                col("value").is_in([-1.25_f64, 0.0, 99.5]),
            ])),
        );
        builder.build().unwrap();
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn engine_rejects_mistyped_cmp_and_every_in_row_policy_literal() {
        let invalid = [
            (
                FilterExpr::Cmp {
                    column: "metric_id".into(),
                    op: super::super::filter::CmpOp::Eq,
                    rhs: Operand::Lit(super::super::LitValue::Json(serde_json::json!("metric-1"))),
                },
                "literal kind `json`",
            ),
            (
                FilterExpr::In {
                    column: "value".into(),
                    values: vec![
                        Operand::from(1.0),
                        Operand::Lit(super::super::LitValue::Json(serde_json::json!(2.0))),
                    ],
                    negated: false,
                },
                "IN operand 1",
            ),
            (
                FilterExpr::Cmp {
                    column: "metric_id".into(),
                    op: super::super::filter::CmpOp::HasKey,
                    rhs: Operand::from("tenant"),
                },
                "operator `HasKey`",
            ),
        ];
        for (predicate, expected) in invalid {
            let pool = sqlx::sqlite::SqlitePoolOptions::new()
                .connect_lazy("sqlite::memory:")
                .unwrap();
            let project =
                DistributedProjectManifest::new("metrics-service").table_schema(metrics());
            let mut builder = GraphqlEngine::from_manifest(&project, pool)
                .unwrap()
                .roles(&["restricted"]);
            insert_permission(
                &mut builder,
                "MetricView",
                "restricted",
                read().all_columns().rows(predicate),
            );
            let error = builder
                .build()
                .err()
                .expect("mistyped row-policy literal must fail");
            assert!(error.to_string().contains(expected), "{error}");
        }
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn sqlite_nocase_can_equate_unequal_code_unit_strings() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let equal: i64 = sqlx::query_scalar("SELECT 'A' = 'a' COLLATE NOCASE")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(equal, 1);
        assert_ne!("A", "a");
    }
}
