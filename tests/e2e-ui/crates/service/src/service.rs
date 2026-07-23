//! Route bundles + GraphQL engine for the e2e-ui fixture.

use blob_domain::BlobGame;
use chat_domain::ChatMessage;
use distributed::graphql::{
    exposed_command, read, ClientReconcile, GraphqlCommands, GraphqlEngine, GraphqlPoolSource,
    IdentityConfig, ModelPermissions, OidcConfig,
};
use distributed::microsvc::{
    ConfigurableOutboxPublisher, HasOutboxStore, HasRepo, Routes, Service,
};
use distributed::{AggregateBuilder, AggregateRepository, Queueable, QueuedRepository};
use e2e_readmodels::{AuthUserView, BlobGameView, ChatMessageView, TodoView};
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
    AggregateRepository<QueuedRepository<R, L>, BlobGame>:
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
        command handlers::commands::force_archive,
        events handlers::events::project_todo,
    );
    let chat = distributed::routes!(
        Routes::new()
            .with_repo(repo.clone().queued_with(locks.clone()).aggregate::<ChatMessage>())
            .with_read_model_store(read_models.clone()),
        command handlers::commands::chat_post,
        // Zitadel Action ingress + on-demand scrape (leaf outbox) + auth_users projector.
        command handlers::ingestors::zitadel,
        command handlers::ingestors::zitadel_scrape,
        event handlers::events::project_chat,
        events handlers::events::project_auth_user,
    );
    let blob = distributed::routes!(
        Routes::new()
            .with_repo(repo.queued_with(locks).aggregate::<BlobGame>())
            .with_read_model_store(read_models),
        command handlers::commands::blob_start,
        command handlers::commands::blob_move,
        command handlers::commands::blob_start_level,
        events handlers::events::project_blob,
    );
    // GraphQL-only public write surface (POST /todo.* must 404 — suite T0 / oidc_pg).
    // Zitadel Action ingress still needs HTTP: those commands are registered above and
    // re-mounted explicitly in `serve_with_oidc` when HTTP command wildcards are off.
    Service::new()
        .named("e2e-ui")
        .without_http_command_routes()
        .routes(todos)
        .routes(chat)
        .routes(blob)
}

/// GraphQL command registry for this fixture — single source of truth for
/// engine mutations **and** `e2e-export-commands` / TypeScript command clients.
pub fn graphql_commands() -> GraphqlCommands {
    use handlers::commands::{
        archive, blob_move, blob_start, blob_start_level, chat_post, complete, create,
        force_archive, payloads, rename, reopen,
    };

    let app_roles = ["user", "admin"];
    // Eventual (async projectors) vs same-tx (blob returns projected row).
    let fact = ClientReconcile::fact();
    let fact_sub = ClientReconcile::fact_subscription();
    let projection = ClientReconcile::projection();

    GraphqlCommands::new()
        .command(
            create::COMMAND,
            exposed_command()
                .field_name("todos_create")
                .input::<create::TodoCreateInput>()
                .output::<create::TodoCreatePayload>()
                .roles(app_roles)
                .client_reconcile(fact.clone()),
        )
        .command(
            complete::COMMAND,
            exposed_command()
                .field_name("todos_complete")
                .input::<complete::TodoCompleteInput>()
                .output::<payloads::TodoStatusPayload>()
                .roles(app_roles)
                .client_reconcile(fact.clone()),
        )
        .command(
            archive::COMMAND,
            exposed_command()
                .field_name("todos_archive")
                .input::<archive::TodoArchiveInput>()
                .output::<payloads::TodoStatusPayload>()
                .roles(app_roles)
                .client_reconcile(fact.clone()),
        )
        .command(
            // Admin-only mutation: appears in admin SDL, not user SDL.
            force_archive::COMMAND,
            exposed_command()
                .field_name("todos_force_archive")
                .input::<force_archive::TodoForceArchiveInput>()
                .output::<force_archive::TodoForceArchivePayload>()
                .roles(["admin"])
                .client_reconcile(fact.clone()),
        )
        .command(
            rename::COMMAND,
            exposed_command()
                .field_name("todos_rename")
                .input::<rename::TodoRenameInput>()
                .output::<rename::TodoRenamePayload>()
                .roles(app_roles)
                .client_reconcile(fact.clone()),
        )
        .command(
            reopen::COMMAND,
            exposed_command()
                .field_name("todos_reopen")
                .input::<reopen::TodoReopenInput>()
                .output::<payloads::TodoStatusPayload>()
                .roles(app_roles)
                .client_reconcile(fact),
        )
        .command(
            chat_post::COMMAND,
            exposed_command()
                .field_name("chat_messages_post")
                .input::<chat_post::ChatPostInput>()
                .output::<chat_post::ChatPostPayload>()
                .roles(app_roles)
                .client_reconcile(fact_sub),
        )
        .command(
            blob_start::COMMAND,
            exposed_command()
                .field_name("blob_games_start")
                .input::<blob_start::BlobStartInput>()
                .output::<blob_start::BlobGamePayload>()
                .roles(app_roles)
                .client_reconcile(projection.clone()),
        )
        .command(
            blob_move::COMMAND,
            exposed_command()
                .field_name("blob_games_move")
                .input::<blob_move::BlobMoveInput>()
                .output::<blob_start::BlobGamePayload>()
                .roles(app_roles)
                .client_reconcile(projection.clone()),
        )
        .command(
            blob_start_level::COMMAND,
            exposed_command()
                .field_name("blob_games_start_level")
                .input::<blob_start_level::BlobStartLevelInput>()
                .output::<blob_start::BlobGamePayload>()
                .roles(app_roles)
                .client_reconcile(projection),
        )
}

/// GraphQL over todos (owner-scoped) + chat_messages (shared room, live subscriptions).
///
/// All write paths are **command mutations** (not read-model writes). Owner/author is
/// always the authenticated session principal. Roles: user, admin.
///
/// Works with SQLite or Postgres pools through [`GraphqlPoolSource`].
pub fn build_graphql_engine(
    pool: impl Into<GraphqlPoolSource>,
    identity: IdentityConfig,
    change_rx: Option<tokio::sync::broadcast::Receiver<distributed::ReadModelChange>>,
) -> Result<GraphqlEngine, String> {
    let commands = graphql_commands();

    let mut b = GraphqlEngine::builder(pool)
        .service_id("e2e-ui")
        .roles(&["user", "admin"])
        // user: only own rows. admin: all owners (UI: /admin all-notes view).
        .model::<TodoView>(
            ModelPermissions::new()
                .grant(
                    "user",
                    read().all_columns().rows(
                        distributed::graphql::col("owner_id")
                            .eq(distributed::graphql::claim("x-user-id")),
                    ),
                )
                .grant("admin", read().all_columns()),
        )
        .model::<ChatMessageView>(
            ModelPermissions::new()
                .grant("user", read().all_columns())
                .grant("admin", read().all_columns()),
        )
        .model::<BlobGameView>(
            ModelPermissions::new()
                .grant(
                    "user",
                    read().all_columns().rows(
                        distributed::graphql::col("owner_id")
                            .eq(distributed::graphql::claim("x-user-id")),
                    ),
                )
                .grant("admin", read().all_columns()),
        )
        // Imported IdP directory (join target for chat.author / blob.owner).
        // Readable by all authenticated roles; writes only via Zitadel projector.
        .model::<AuthUserView>(
            ModelPermissions::new()
                .grant("user", read().all_columns())
                .grant("admin", read().all_columns()),
        )
        .commands(commands)
        .identity(identity)
        // GraphiQL is a local template convenience. Disable with GRAPHIQL=0
        // (never ship a public edge with GraphiQL + DevHeaders).
        .graphiql(graphiql_enabled());
    if let Some(rx) = change_rx {
        b = b.change_stream(rx);
    }
    b.build().map_err(|e| e.to_string())
}

pub fn dev_identity() -> IdentityConfig {
    IdentityConfig::dev_headers()
}

/// GraphiQL IDE: on by default for the fixture; set `GRAPHIQL=0` to disable.
pub fn graphiql_enabled() -> bool {
    match std::env::var("GRAPHIQL") {
        Ok(v) => {
            let v = v.trim();
            !(v == "0" || v.eq_ignore_ascii_case("false") || v.eq_ignore_ascii_case("off"))
        }
        Err(_) => true,
    }
}

/// Peel accidental outer quotes from env values (Make-include / double-wrap pollution).
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

/// Prefer OidcBearer when `OIDC_ISSUER` + `OIDC_AUDIENCE` are set; else DevHeaders.
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
    // Accept client_id as extra audience when present (human OIDC access tokens).
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
    IdentityConfig::oidc_bearer(oidc)
}
