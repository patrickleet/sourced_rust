//! Route bundles + GraphQL engine for the e2e-ui fixture.

use std::collections::BTreeMap;
use std::sync::Arc;

use blob_domain::BlobGame;
use chat_domain::ChatMessage;
use distributed::graphql::{
    build_surface, read, surface_for_application, typed_command, DistributedClientSurfaceExport,
    Fact, GraphqlEngine, GraphqlPoolSource, IdentityConfig, ModelPermissions, OidcConfig,
    Projected, RoleGrant, SurfaceOptions, SurfaceProjector,
};
use distributed::microsvc::{
    ConfigurableOutboxPublisher, HasOutboxStore, HasRepo, Routes, Service,
};
use distributed::{
    command_confirmations, command_effects, command_input_defaults, AggregateBuilder,
    AggregateRepository, InMemoryLockManager, InMemoryRepository, LockError, LockManager,
    Queueable, QueuedRepository,
};
use e2e_readmodels::{AuthUserView, BlobGameView, ChatMessageView, TodoView};
use todo_domain::Todo;

use crate::bounds::{EventStore, Locks, ReadStore};
use crate::handlers;

// Stable only for this local copyable fixture. Real deployments must inject
// their own per-deployment key rather than copying this development value.
const E2E_PROTOCOL_TOKEN_KEY: [u8; 32] = [0xe2; 32];

/// Stable normal-application surface shared by user and admin sessions.
pub const DISTRIBUTED_CLIENT_SURFACE: &str = "fieldnote";
/// Stable elevated surface for routes that intentionally include admin-only fields.
pub const DISTRIBUTED_ADMIN_CLIENT_SURFACE: &str = "fieldnote-admin";

#[derive(Clone, Default)]
struct ClientSurfaceLocks(Arc<InMemoryLockManager>);

impl LockManager for ClientSurfaceLocks {
    type Lock = distributed::InMemoryLock;

    fn get_lock(&self, id: &str) -> Result<Arc<Self::Lock>, LockError> {
        self.0.get_lock(id)
    }
}

fn todo_projector() -> SurfaceProjector {
    SurfaceProjector::new("project_todo")
        .facts(handlers::events::project_todo::EVENTS.iter().copied())
        .models(["TodoView"])
        .change_epoch("e2e-ui-todos-v1")
}

fn chat_projector() -> SurfaceProjector {
    SurfaceProjector::new("project_chat")
        .facts([handlers::events::project_chat::EVENT])
        .models(["ChatMessageView"])
        .change_epoch("e2e-ui-chat-v1")
}

fn blob_projector() -> SurfaceProjector {
    SurfaceProjector::new("project_blob")
        .facts(handlers::events::project_blob::EVENTS.iter().copied())
        .models(["BlobGameView"])
        .change_epoch("e2e-ui-blob-v1")
}

fn client_grants() -> BTreeMap<String, BTreeMap<String, RoleGrant>> {
    let all_models = || {
        BTreeMap::from([
            ("AuthUserView".into(), RoleGrant::all_columns()),
            ("BlobGameView".into(), RoleGrant::all_columns()),
            ("ChatMessageView".into(), RoleGrant::all_columns()),
            ("TodoView".into(), RoleGrant::all_columns()),
        ])
    };
    let mut user = all_models();
    user.insert(
        "TodoView".into(),
        RoleGrant::all_columns().rows(
            distributed::graphql::col("owner_id").eq(distributed::graphql::claim("x-user-id")),
        ),
    );
    user.insert(
        "BlobGameView".into(),
        RoleGrant::all_columns().rows(
            distributed::graphql::col("owner_id").eq(distributed::graphql::claim("x-user-id")),
        ),
    );
    BTreeMap::from([("user".into(), user), ("admin".into(), all_models())])
}

fn pool_free_client_surface(application: &str, roles: &[&str]) -> DistributedClientSurfaceExport {
    let project = e2e_readmodels::distributed_manifest();
    let repository = InMemoryRepository::new();
    let service = build_service(
        repository.clone(),
        ClientSurfaceLocks::default(),
        repository,
    );
    let full = build_surface(&project.tables, &SurfaceOptions::sqlite())
        .expect("e2e-ui client Surface should build")
        .with_service(&service)
        .expect("e2e-ui typed Service inventory should bind")
        .with_projectors([todo_projector(), chat_projector(), blob_projector()])
        .expect("e2e-ui projector topology should bind");
    let roles = roles
        .iter()
        .map(|role| (*role).to_string())
        .collect::<Vec<_>>();
    let selected = surface_for_application(&full, application, &roles, &client_grants())
        .expect("e2e-ui application Surface should select");
    DistributedClientSurfaceExport::from_project(&project, selected)
        .expect("e2e-ui application Surface should export")
}

/// Pool-free normal application export consumed by `dctl client-manifest`.
pub fn distributed_client_surface() -> DistributedClientSurfaceExport {
    pool_free_client_surface(DISTRIBUTED_CLIENT_SURFACE, &["user", "admin"])
}

/// Pool-free elevated application export for admin-only routes.
pub fn distributed_admin_client_surface() -> DistributedClientSurfaceExport {
    pool_free_client_surface(DISTRIBUTED_ADMIN_CLIENT_SURFACE, &["admin"])
}

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
    use handlers::commands::{
        archive, blob_move, blob_start, blob_start_level, chat_post, complete, create,
        force_archive, payloads, rename, reopen,
    };

    let app_roles = ["user", "admin"];
    let todo_projection = todo_projector();
    let todos = Routes::new()
        .with_repo(repo.clone().queued_with(locks.clone()).aggregate::<Todo>())
        .with_read_model_store(read_models.clone())
        .typed_command(
            typed_command::<create::TodoCreateInput, Fact<create::TodoCreatePayload>>(
                create::COMMAND,
            )
            .field_name("todos_create")
            .roles(app_roles)
            .input_defaults(command_input_defaults! {
                input: create::TodoCreateInput;
                default input.todo_id = uuid_v7();
            })
            .effects(command_effects! {
                input: create::TodoCreateInput;
                upsert e2e_readmodels::models::todo_view::TodoView {
                    key { todo_id: input.todo_id },
                    set {
                        owner_id: trusted("x-user-id"),
                        title: input.title,
                        status: "open"
                    }
                };
            })
            .confirmations(command_confirmations! {
                input: create::TodoCreateInput;
                confirm todo_projection -> e2e_readmodels::models::todo_view::TodoView {
                    key { todo_id: input.todo_id }
                };
            }),
        )
        .handle(create::handle)
        .typed_command(
            typed_command::<rename::TodoRenameInput, Fact<rename::TodoRenamePayload>>(
                rename::COMMAND,
            )
            .field_name("todos_rename")
            .roles(app_roles)
            .effects(command_effects! {
                input: rename::TodoRenameInput;
                patch e2e_readmodels::models::todo_view::TodoView {
                    key { todo_id: input.todo_id },
                    set { title: input.title }
                };
            })
            .confirmations(command_confirmations! {
                input: rename::TodoRenameInput;
                confirm todo_projection -> e2e_readmodels::models::todo_view::TodoView {
                    key { todo_id: input.todo_id }
                };
            }),
        )
        .handle(rename::handle)
        .typed_command(
            typed_command::<complete::TodoCompleteInput, Fact<payloads::TodoStatusPayload>>(
                complete::COMMAND,
            )
            .field_name("todos_complete")
            .roles(app_roles)
            .effects(command_effects! {
                input: complete::TodoCompleteInput;
                patch e2e_readmodels::models::todo_view::TodoView {
                    key { todo_id: input.todo_id },
                    set { status: "completed" }
                };
            })
            .confirmations(command_confirmations! {
                input: complete::TodoCompleteInput;
                confirm todo_projection -> e2e_readmodels::models::todo_view::TodoView {
                    key { todo_id: input.todo_id }
                };
            }),
        )
        .handle(complete::handle)
        .typed_command(
            typed_command::<reopen::TodoReopenInput, Fact<reopen::TodoReopenPayload>>(
                reopen::COMMAND,
            )
            .field_name("todos_reopen")
            .roles(app_roles)
            .effects(command_effects! {
                input: reopen::TodoReopenInput;
                patch e2e_readmodels::models::todo_view::TodoView {
                    key { todo_id: input.todo_id },
                    set { status: "open" }
                };
            })
            .confirmations(command_confirmations! {
                input: reopen::TodoReopenInput;
                confirm todo_projection -> e2e_readmodels::models::todo_view::TodoView {
                    key { todo_id: input.todo_id }
                };
            }),
        )
        .handle(reopen::handle)
        .typed_command(
            typed_command::<archive::TodoArchiveInput, Fact<archive::TodoArchivePayload>>(
                archive::COMMAND,
            )
            .field_name("todos_archive")
            .roles(app_roles)
            .effects(command_effects! {
                input: archive::TodoArchiveInput;
                patch e2e_readmodels::models::todo_view::TodoView {
                    key { todo_id: input.todo_id },
                    set { status: "archived" }
                };
            })
            .confirmations(command_confirmations! {
                input: archive::TodoArchiveInput;
                confirm todo_projection -> e2e_readmodels::models::todo_view::TodoView {
                    key { todo_id: input.todo_id }
                };
            }),
        )
        .handle(archive::handle)
        .typed_command(
            typed_command::<
                force_archive::TodoForceArchiveInput,
                Fact<force_archive::TodoForceArchivePayload>,
            >(force_archive::COMMAND)
            .field_name("todos_force_archive")
            .roles(["admin"])
            .effects(command_effects! {
                input: force_archive::TodoForceArchiveInput;
                patch e2e_readmodels::models::todo_view::TodoView {
                    key { todo_id: input.todo_id },
                    set { status: "archived" }
                };
            })
            .confirmations(command_confirmations! {
                input: force_archive::TodoForceArchiveInput;
                confirm todo_projection -> e2e_readmodels::models::todo_view::TodoView {
                    key { todo_id: input.todo_id }
                };
            }),
        )
        .handle(force_archive::handle)
        .causal_projector::<todo_domain::TodoFact>(todo_projection)
        .model::<TodoView>()
        .handle(handlers::events::project_todo::handle);

    let chat_projection = chat_projector();
    let chat = Routes::new()
        .with_repo(
            repo.clone()
                .queued_with(locks.clone())
                .aggregate::<ChatMessage>(),
        )
        .with_read_model_store(read_models.clone())
        .typed_command(
            typed_command::<chat_post::ChatPostInput, Fact<chat_post::ChatPostPayload>>(
                chat_post::COMMAND,
            )
            .field_name("chat_messages_post")
            .roles(app_roles)
            .effects(command_effects! {
                input: chat_post::ChatPostInput;
                upsert e2e_readmodels::models::chat_message_view::ChatMessageView {
                    key { message_id: input.message_id },
                    set {
                        room_id: input.room_id,
                        author_id: trusted("x-user-id"),
                        body: input.body,
                        created_at: input.created_at
                    }
                };
            })
            .confirmations(command_confirmations! {
                input: chat_post::ChatPostInput;
                confirm chat_projection -> e2e_readmodels::models::chat_message_view::ChatMessageView {
                    key { message_id: input.message_id }
                };
            }),
        )
        .handle(chat_post::handle)
        // Zitadel Action ingress + on-demand scrape remain non-GraphQL
        // integration commands.
        .command(handlers::ingestors::zitadel::COMMAND)
        .guarded(
            handlers::ingestors::zitadel::guard,
            handlers::ingestors::zitadel::handle,
        )
        .command(handlers::ingestors::zitadel_scrape::COMMAND)
        .guarded(
            handlers::ingestors::zitadel_scrape::guard,
            handlers::ingestors::zitadel_scrape::handle,
        )
        .causal_projector::<chat_domain::ChatMessagePosted>(chat_projection)
        .model::<ChatMessageView>()
        .handle(handlers::events::project_chat::handle)
        .events(handlers::events::project_auth_user::EVENTS)
        .guarded(
            handlers::events::project_auth_user::guard,
            handlers::events::project_auth_user::handle,
        );

    let blob_projection = blob_projector();
    let blob = Routes::new()
        .with_repo(repo.queued_with(locks).aggregate::<BlobGame>())
        .with_read_model_store(read_models)
        .typed_command(
            typed_command::<blob_start::BlobStartInput, Projected<BlobGameView>>(
                blob_start::COMMAND,
            )
            .field_name("blob_games_start")
            .roles(app_roles),
        )
        .handle(blob_start::handle)
        .typed_command(
            typed_command::<blob_move::BlobMoveInput, Projected<BlobGameView>>(blob_move::COMMAND)
                .field_name("blob_games_move")
                .roles(app_roles),
        )
        .handle(blob_move::handle)
        .typed_command(
            typed_command::<blob_start_level::BlobStartLevelInput, Projected<BlobGameView>>(
                blob_start_level::COMMAND,
            )
            .field_name("blob_games_start_level")
            .roles(app_roles),
        )
        .handle(blob_start_level::handle)
        .causal_projector::<blob_domain::BlobGameFact>(blob_projection)
        .model::<BlobGameView>()
        .handle(handlers::events::project_blob::handle);

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

/// GraphQL over todos (owner-scoped) + chat_messages (shared room, live subscriptions).
///
/// All write paths are **command mutations** (not read-model writes). Owner/author is
/// always the authenticated session principal. Roles: user, admin.
///
/// Works with SQLite or Postgres pools through [`GraphqlPoolSource`].
pub fn build_graphql_engine(
    pool: impl Into<GraphqlPoolSource>,
    service: &Service,
    identity: IdentityConfig,
    change_rx: Option<tokio::sync::broadcast::Receiver<distributed::ReadModelChange>>,
) -> Result<GraphqlEngine, String> {
    let mut b = GraphqlEngine::builder(pool)
        .protocol_token_key(E2E_PROTOCOL_TOKEN_KEY)
        .roles(&["user", "admin"])
        .client_application_surface(DISTRIBUTED_CLIENT_SURFACE, ["user", "admin"])
        .client_application_surface(DISTRIBUTED_ADMIN_CLIENT_SURFACE, ["admin"])
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
        .service(service)
        .client_projectors([todo_projector(), chat_projector(), blob_projector()])
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

#[cfg(test)]
mod client_surface_tests {
    use super::*;

    #[test]
    fn pool_free_user_and_admin_exports_compile_real_manifests() {
        distributed_client_surface()
            .manifest()
            .expect("normal application client manifest");
        distributed_admin_client_surface()
            .manifest()
            .expect("elevated application client manifest");
    }
}
