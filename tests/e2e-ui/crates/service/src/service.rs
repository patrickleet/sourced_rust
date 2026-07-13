//! Route bundles + GraphQL engine for the e2e-ui fixture.

use chat_domain::ChatMessage;
use distributed::graphql::{
    select, GraphqlEngine, IdentityConfig, ModelPermissions, OidcConfig,
};
use distributed::microsvc::{
    ConfigurableOutboxPublisher, HasOutboxStore, HasRepo, Routes, Service,
};
use distributed::{AggregateBuilder, AggregateRepository, Queueable, QueuedRepository};
use e2e_readmodels::{ChatMessageView, TodoView};
use sqlx::SqlitePool;
use todo_domain::Todo;

use crate::bounds::{EventStore, Locks, ReadStore};
use crate::handlers;

/// Full service: todo + chat commands and projectors.
pub fn build_service<R, L, S>(repo: R, locks: L, read_models: S) -> Service
where
    R: EventStore,
    L: Locks,
    S: ReadStore,
    QueuedRepository<R, L>: Clone
        + AggregateBuilder
        + HasOutboxStore
        + distributed::TransactionalCommit
        + Send
        + Sync
        + 'static,
    AggregateRepository<QueuedRepository<R, L>, Todo>:
        HasRepo + HasOutboxStore + ConfigurableOutboxPublisher + Send + Sync + 'static,
    AggregateRepository<QueuedRepository<R, L>, ChatMessage>:
        HasRepo + HasOutboxStore + ConfigurableOutboxPublisher + Send + Sync + 'static,
{
    let todos = distributed::routes!(
        Routes::new()
            .with_repo(repo.clone().queued_with(locks.clone()).aggregate::<Todo>())
            .with_read_model_store(read_models.clone()),
        command handlers::commands::create,
        command handlers::commands::rename,
        command handlers::commands::complete,
        command handlers::commands::reopen,
        command handlers::commands::archive,
        events handlers::events::project_todo,
    );
    let chat = distributed::routes!(
        Routes::new()
            .with_repo(repo.queued_with(locks).aggregate::<ChatMessage>())
            .with_read_model_store(read_models),
        command handlers::commands::chat_post,
        event handlers::events::project_chat,
    );
    Service::new().named("e2e-ui").routes(todos).routes(chat)
}

/// GraphQL over todos (owner-scoped) + chat_messages (shared room, live subscriptions).
///
/// Pass `change_stream` from `SqliteRepository::read_model_changes()` so
/// subscription fields re-run after projector commits.
pub fn build_graphql_engine(
    pool: SqlitePool,
    identity: IdentityConfig,
    change_rx: Option<tokio::sync::broadcast::Receiver<distributed::ReadModelChange>>,
) -> Result<GraphqlEngine, String> {
    let mut b = GraphqlEngine::builder(pool)
        .roles(&["user", "admin"])
        .model::<TodoView>(
            ModelPermissions::new()
                .role(
                    "user",
                    select().all_columns().filter(
                        distributed::graphql::col("owner_id")
                            .eq(distributed::graphql::claim("x-user-id")),
                    ),
                )
                .role("admin", select().all_columns()),
        )
        .model::<ChatMessageView>(
            ModelPermissions::new()
                // Shared lobby: any authenticated user can see all messages.
                .role("user", select().all_columns())
                .role("admin", select().all_columns()),
        )
        .identity(identity)
        .graphiql(true);
    if let Some(rx) = change_rx {
        b = b.change_stream(rx);
    }
    b.build().map_err(|e| e.to_string())
}

pub fn dev_identity() -> IdentityConfig {
    IdentityConfig::dev_headers()
}

pub fn identity_from_env() -> IdentityConfig {
    let iss = std::env::var("OIDC_ISSUER").unwrap_or_default();
    let aud = std::env::var("OIDC_AUDIENCE").unwrap_or_default();
    if iss.is_empty() || aud.is_empty() {
        return dev_identity();
    }
    oidc_bearer_config(iss, aud, std::env::var("OIDC_JWKS_URI").ok(), None)
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
    oidc.claim_map.engine_roles = vec!["user".into(), "admin".into()];
    oidc.claim_map.role_claims = vec![
        "groups".into(),
        "roles".into(),
        "realm_access.roles".into(),
    ];
    IdentityConfig::oidc_bearer(oidc)
}
