use std::sync::Arc;

use distributed::graphql::{
    build_surface, surface_for_application_contract, DistributedClientSurfaceExport, GraphqlEngine,
    GraphqlPoolSource, IdentityConfig, OidcConfig, SurfaceOptions,
};
use distributed::microsvc::Service;
use distributed::{InMemoryLockManager, InMemoryRepository, LockError, LockManager};
use e2e_readmodels::{AuthUsers, BlobGames, ChatMessages, Todos};

use crate::application::{
    DISTRIBUTED_ADMIN_CLIENT_SURFACE, DISTRIBUTED_CLIENT_SURFACE, DISTRIBUTED_PUBLIC_CLIENT_SURFACE,
};
use crate::modules::projections;

// Stable only for this local copyable fixture. Real deployments must inject
// their own per-deployment key rather than copying this development value.
const E2E_PROTOCOL_TOKEN_KEY: [u8; 32] = [0xe2; 32];

#[derive(Clone, Default)]
pub(crate) struct ClientSurfaceLocks(Arc<InMemoryLockManager>);

impl LockManager for ClientSurfaceLocks {
    type Lock = distributed::InMemoryLock;

    fn get_lock(&self, id: &str) -> Result<Arc<Self::Lock>, LockError> {
        self.0.get_lock(id)
    }
}

/// GraphQL over todos + chat + blob + AuthUsers.
pub fn build_graphql_engine(
    pool: impl Into<GraphqlPoolSource>,
    service: &Service,
    identity: IdentityConfig,
    change_rx: Option<tokio::sync::broadcast::Receiver<distributed::ReadModelChange>>,
) -> Result<GraphqlEngine, String> {
    build_graphql_engine_with_graphiql(pool, service, identity, change_rx, graphiql_enabled())
}

pub(crate) fn build_graphql_engine_with_graphiql(
    pool: impl Into<GraphqlPoolSource>,
    service: &Service,
    identity: IdentityConfig,
    change_rx: Option<tokio::sync::broadcast::Receiver<distributed::ReadModelChange>>,
    graphiql: bool,
) -> Result<GraphqlEngine, String> {
    let projections = projections::projection_owners();
    let mut b = GraphqlEngine::builder(pool)
        .protocol_token_key(E2E_PROTOCOL_TOKEN_KEY)
        .roles(&["user", "admin", "anonymous"])
        .client_application_surface_with_schema_roles(
            DISTRIBUTED_CLIENT_SURFACE,
            ["admin", "user"],
            ["user"],
        )
        .client_application_surface(DISTRIBUTED_ADMIN_CLIENT_SURFACE, ["admin"], ["admin"])
        .client_application_surface(DISTRIBUTED_PUBLIC_CLIENT_SURFACE, ["anonymous"], ["anonymous"])
        .model::<Todos>(Todos::permissions())
        .model::<ChatMessages>(ChatMessages::permissions())
        .model::<BlobGames>(BlobGames::permissions())
        .model::<AuthUsers>(AuthUsers::permissions())
        .service(service)
        .client_projection_owners([
            projections.todo.into(),
            projections.chat.into(),
            projections.blob.into(),
        ])
        .identity(identity)
        .graphiql(graphiql);
    if let Some(rx) = change_rx {
        b = b.change_stream(rx);
    }
    b.build().map_err(|e| e.to_string())
}

fn pool_free_client_surface(application: &str, roles: &[&str]) -> DistributedClientSurfaceExport {
    pool_free_client_surface_contract(application, roles, roles)
}

fn pool_free_client_surface_contract(
    application: &str,
    eligible_roles: &[&str],
    schema_roles: &[&str],
) -> DistributedClientSurfaceExport {
    let project = e2e_readmodels::distributed_manifest();
    let repository = InMemoryRepository::new();
    let service = crate::modules::compose::build_service(
        repository.clone(),
        ClientSurfaceLocks::default(),
        repository,
    );
    let projections = projections::projection_owners();
    let full = build_surface(&project.tables, &SurfaceOptions::sqlite())
        .expect("e2e-ui client Surface should build")
        .with_projection_owners([
            projections.todo.into(),
            projections.chat.into(),
            projections.blob.into(),
        ])
        .expect("e2e-ui projector topology should bind")
        .with_service(&service)
        .expect("e2e-ui typed Service inventory should bind");
    let eligible = eligible_roles
        .iter()
        .map(|role| (*role).to_string())
        .collect::<Vec<_>>();
    let schema = schema_roles
        .iter()
        .map(|role| (*role).to_string())
        .collect::<Vec<_>>();
    let grants = e2e_readmodels::application_grants();
    let selected = surface_for_application_contract(
        &full,
        application,
        &eligible,
        &schema,
        &grants,
    )
    .expect("e2e-ui application Surface should select");
    DistributedClientSurfaceExport::from_selected("e2e-ui", selected)
        .expect("e2e-ui application Surface should export")
}

/// Pool-free normal application export consumed by `distributed client-manifest`.
pub fn distributed_client_surface() -> DistributedClientSurfaceExport {
    pool_free_client_surface_contract(
        DISTRIBUTED_CLIENT_SURFACE,
        &["admin", "user"],
        &["user"],
    )
}

pub fn distributed_admin_client_surface() -> DistributedClientSurfaceExport {
    pool_free_client_surface(DISTRIBUTED_ADMIN_CLIENT_SURFACE, &["admin"])
}

pub fn distributed_public_client_surface() -> DistributedClientSurfaceExport {
    pool_free_client_surface(DISTRIBUTED_PUBLIC_CLIENT_SURFACE, &["anonymous"])
}

pub fn dev_identity() -> IdentityConfig {
    IdentityConfig::dev_headers()
}

pub fn graphiql_enabled() -> bool {
    match std::env::var("GRAPHIQL") {
        Ok(v) => {
            let v = v.trim();
            !(v == "0" || v.eq_ignore_ascii_case("false") || v.eq_ignore_ascii_case("off"))
        }
        Err(_) => true,
    }
}

fn env_clean(name: &str) -> String {
    let mut s = std::env::var(name).unwrap_or_default().trim().to_string();
    for _ in 0..2 {
        if s.len() >= 2
            && ((s.starts_with('\'') && s.ends_with('\''))
                || (s.starts_with('"') && s.ends_with('"')))
        {
            s = s[1..s.len() - 1].trim().to_string();
        } else {
            break;
        }
    }
    s
}

pub fn identity_from_env() -> IdentityConfig {
    let iss = env_clean("OIDC_ISSUER");
    let aud = env_clean("OIDC_AUDIENCE");
    if iss.is_empty() || aud.is_empty() {
        eprintln!("e2e-ui: OIDC_* unset — using DevHeaders (local only)");
        return dev_identity();
    }
    let jwks = env_clean("OIDC_JWKS_URI");
    eprintln!("e2e-ui: OidcBearer issuer={iss} audience={aud}");
    oidc_bearer_config(
        iss,
        aud,
        if jwks.is_empty() { None } else { Some(jwks) },
        None,
    )
}

pub fn oidc_bearer_config(
    issuer: impl Into<String>,
    audience: impl Into<String>,
    jwks_uri: Option<String>,
    static_jwks: Option<String>,
) -> IdentityConfig {
    let mut oidc = OidcConfig::new(issuer, audience);
    if let Some(uri) = jwks_uri.filter(|s| !s.is_empty()) {
        oidc.jwks_uri = Some(uri);
    }
    if let Some(jwks) = static_jwks {
        oidc = oidc.with_static_jwks(jwks);
    }
    let cid = env_clean("OIDC_CLIENT_ID");
    if !cid.is_empty() {
        oidc.extra_audiences = vec![cid];
    }
    oidc.claim_map.engine_roles = vec!["user".into(), "admin".into()];
    oidc.claim_map.role_claims = vec![
        "groups".into(),
        "roles".into(),
        "realm_access.roles".into(),
        "urn:zitadel:iam:org:project:roles".into(),
    ];
    oidc.require_auth = false;
    IdentityConfig::oidc_bearer(oidc)
}
