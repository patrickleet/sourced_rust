//! Route bundles + GraphQL engine for the e2e-ui fixture.

use std::sync::Arc;

use blob_domain::{
    BlobGame, BlobLevelStartedDomainEvent, BlobMovedDomainEvent, BlobStartedDomainEvent,
};
use chat_domain::{ChatMessage, ChatMessagePostedDomainEvent};
use distributed::graphql::{
    build_surface, typed_command, Eventual, CommandProjectionPreview,
    CommandProjectionPreviewSource, DistributedClientSurfaceExport, GraphqlEngine,
    GraphqlPoolSource, IdentityConfig, OidcConfig, Atomic, SurfaceDirectProjection,
    SurfaceModeledProjection, SurfaceOptions, SurfaceProjector,
};
use distributed::microsvc::{
    ConfigurableOutboxPublisher, HasOutboxStore, HasRepo, Routes, Service,
};
use distributed::projection::catalog::{ProjectionBindingActivation, ProjectionCatalog};
use distributed::projection::lower::ProjectionDescriptor;
use distributed::projection::placement::{
    ProjectionBinding, ProjectionBindingState, ProjectionEpoch, ProjectionExecutorRoute,
    ProjectionOutput, ProjectionOwner, ProjectionPhysicalTopology, ProjectionSourceBinding,
    PROJECTION_PARTITION_CODEC_VERSION,
};
use distributed::projection_protocol::ProjectorTopologyId;
use distributed::{
    command_input_defaults, AggregateBuilder, AggregateRepository, InMemoryLockManager,
    InMemoryRepository, LockError, LockManager, ProjectionEnvelopeField, Queueable,
    QueuedRepository, RelationalReadModel,
};
use e2e_projections::{BLOB_GAMES, CHAT_MESSAGES, TODOS};
use e2e_readmodels::{AuthUsers, BlobGames, ChatMessages, Todos};
use todo_domain::{
    Todo, TodoArchivedDomainEvent, TodoCompletedDomainEvent, TodoCreatedDomainEvent,
    TodoForceArchivedDomainEvent, TodoPurgedDomainEvent, TodoRenamedDomainEvent,
    TodoReopenedDomainEvent,
};

use crate::bounds::{EventStore, Locks, ReadStore};
use crate::handlers;

// Stable only for this local copyable fixture. Real deployments must inject
// their own per-deployment key rather than copying this development value.
const E2E_PROTOCOL_TOKEN_KEY: [u8; 32] = [0xe2; 32];

/// Stable normal-application surface shared by user and admin sessions.
pub const DISTRIBUTED_CLIENT_SURFACE: &str = "e2e-ui";
/// Stable elevated surface for routes that intentionally include admin-only fields.
pub const DISTRIBUTED_ADMIN_CLIENT_SURFACE: &str = "e2e-ui-admin";
/// Unauthenticated public surface (lobby message peek).
pub const DISTRIBUTED_PUBLIC_CLIENT_SURFACE: &str = "e2e-ui-public";

#[derive(Clone, Default)]
struct ClientSurfaceLocks(Arc<InMemoryLockManager>);

impl LockManager for ClientSurfaceLocks {
    type Lock = distributed::InMemoryLock;

    fn get_lock(&self, id: &str) -> Result<Arc<Self::Lock>, LockError> {
        self.0.get_lock(id)
    }
}

#[derive(Clone)]
struct ProjectionOwners {
    todo: SurfaceProjector,
    chat: SurfaceProjector,
    blob: SurfaceDirectProjection,
}

fn projection_output<M: RelationalReadModel>() -> ProjectionOutput {
    let schema = M::schema().clone();
    ProjectionOutput::try_new(schema.model_name.clone(), schema.table_name.clone(), schema)
        .expect("canonical e2e-ui projection output")
}

fn physical_topology(name: &str, digest: u8) -> ProjectionPhysicalTopology {
    ProjectionPhysicalTopology::from_protocol(
        &ProjectorTopologyId::new(1, name, [digest; 32])
            .expect("canonical e2e-ui physical topology"),
    )
}

fn modeled_projection<D>(
    descriptor: ProjectionDescriptor<D>,
    catalog: &ProjectionCatalog,
    active: &distributed::projection::catalog::ActiveProjectionBindings,
    binding: &ProjectionBinding,
) -> SurfaceModeledProjection {
    SurfaceModeledProjection::try_from_descriptor(descriptor, catalog, active, binding.id())
        .expect("modeled projection should resolve through the active catalog")
}

fn projection_owners() -> ProjectionOwners {
    let source = || {
        ProjectionSourceBinding::try_new("e2e-ui-domain", "ordered-domain-events", 1)
            .expect("canonical e2e-ui domain source")
    };
    let owner = |name| ProjectionOwner::try_new(name).expect("canonical projection owner");

    let todo_binding = ProjectionBinding::materialize_eventual(
        TODOS.eventual(),
        source(),
        owner("project_todos"),
        "distributed-projection-partition",
        PROJECTION_PARTITION_CODEC_VERSION,
        vec![projection_output::<Todos>()],
        Vec::new(),
        Some(physical_topology("project_todos", 0x20)),
    )
    .expect("Todo projection binding");
    let chat_binding = ProjectionBinding::materialize_eventual(
        CHAT_MESSAGES.eventual(),
        source(),
        owner("project_chat_messages"),
        "distributed-projection-partition",
        PROJECTION_PARTITION_CODEC_VERSION,
        vec![projection_output::<ChatMessages>()],
        Vec::new(),
        Some(physical_topology("project_chat_messages", 0x21)),
    )
    .expect("Chat projection binding");
    let blob_binding = ProjectionBinding::materialize_direct(
        BLOB_GAMES.direct(),
        source(),
        owner("project_blob"),
        "distributed-projection-partition",
        PROJECTION_PARTITION_CODEC_VERSION,
        vec![projection_output::<BlobGames>()],
        Vec::new(),
        Some(physical_topology("project_blob", 0x22)),
    )
    .expect("Blob projection binding");
    // Blob projected commands stage the mutation-derived row in the handler
    // (`readmodel(row).commit()?.atomic()`). Binding/catalog still own
    // ownership, replay, and async projection for BLOB_GAMES.

    let catalog = ProjectionCatalog::try_new(vec![
        todo_binding.clone(),
        chat_binding.clone(),
        blob_binding.clone(),
    ])
    .expect("deployment-wide projection catalog");
    let activation = |binding: &ProjectionBinding, epoch: &str| {
        ProjectionBindingActivation::new(
            binding.id(),
            binding.program_id(),
            ProjectionEpoch::new(epoch).expect("canonical projection epoch"),
            ProjectionBindingState::Active,
            Some(
                ProjectionExecutorRoute::local("e2e-ui").expect("canonical local projection route"),
            ),
        )
    };
    let active = catalog
        .activate(
            vec![
                activation(&todo_binding, "e2e-ui-todos-v2"),
                activation(&chat_binding, "e2e-ui-chat-v2"),
                activation(&blob_binding, "e2e-ui-blob-v2"),
            ],
            None,
        )
        .expect("non-overlapping active projection catalog");

    // Runtime mounts are mutation-backed: descriptor program factories must
    // match the mutation rewrite programs (real path, not digest theater).
    
    
    

    ProjectionOwners {
        todo: SurfaceProjector::new("project_todos").modeled(modeled_projection(
            TODOS,
            &catalog,
            &active,
            &todo_binding,
        )),
        chat: SurfaceProjector::new("project_chat_messages").modeled(modeled_projection(
            CHAT_MESSAGES,
            &catalog,
            &active,
            &chat_binding,
        )),
        blob: SurfaceDirectProjection::new("project_blob").modeled(modeled_projection(
            BLOB_GAMES,
            &catalog,
            &active,
            &blob_binding,
        )),
    }
}

fn pool_free_client_surface(application: &str, roles: &[&str]) -> DistributedClientSurfaceExport {
    pool_free_client_surface_contract(application, roles, roles)
}

fn pool_free_client_surface_contract(
    application: &str,
    eligible_roles: &[&str],
    schema_roles: &[&str],
) -> DistributedClientSurfaceExport {
    use distributed::graphql::surface_for_application_contract;

    let project = e2e_readmodels::distributed_manifest();
    let repository = InMemoryRepository::new();
    let service = build_service(
        repository.clone(),
        ClientSurfaceLocks::default(),
        repository,
    );
    let projections = projection_owners();
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
    // Schema grants: only schema_roles need entries in the map.
    let grants = e2e_readmodels::application_grants();
    let selected = surface_for_application_contract(
        &full,
        application,
        &eligible,
        &schema,
        &grants,
    )
    .expect("e2e-ui application Surface should select");
    DistributedClientSurfaceExport::from_project(&project, selected)
        .expect("e2e-ui application Surface should export")
}

/// Pool-free normal application export consumed by `dctl client-manifest`.
///
/// **Eligible roles** are `admin` + `user` so multi-role admin principals can
/// open the normal app client. **Schema privilege** is the `user` grant set
/// only so owner-scoped models (`Todos`, `BlobGames`) keep a **client-portable**
/// row policy (`owner_id = claim(x-user-id)`) for optimistic list inserts.
///
/// Server-side admin GraphQL grants are unchanged — an admin session still
/// receives unrestricted query results via the concrete admin role surface.
/// Elevated all-rows views / force-archive stay on
/// [`distributed_admin_client_surface`].
pub fn distributed_client_surface() -> DistributedClientSurfaceExport {
    pool_free_client_surface_contract(
        DISTRIBUTED_CLIENT_SURFACE,
        &["admin", "user"],
        &["user"],
    )
}

/// Pool-free elevated application export for admin-only routes.
pub fn distributed_admin_client_surface() -> DistributedClientSurfaceExport {
    pool_free_client_surface(DISTRIBUTED_ADMIN_CLIENT_SURFACE, &["admin"])
}

/// Pool-free public (anonymous) application export for unauthenticated lobby peeks.
pub fn distributed_public_client_surface() -> DistributedClientSurfaceExport {
    pool_free_client_surface(DISTRIBUTED_PUBLIC_CLIENT_SURFACE, &["anonymous"])
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
        blob_move, blob_start, blob_start_level, chat_post, payloads, todo_archive, todo_complete,
        todo_create, todo_force_archive, todo_purge, todo_rename, todo_reopen,
    };

    let app_roles = ["user", "admin"];
    let projections = projection_owners();
    let todos = Routes::new()
        .with_repo(repo.clone().queued_with(locks.clone()).aggregate::<Todo>())
        .with_read_model_store(read_models.clone())
        .typed_command(
            typed_command::<todo_create::TodoCreateInput, Eventual<todo_create::TodoCreatePayload>>(
                todo_create::COMMAND,
            )
            .field_name("todos_create")
            .roles(app_roles)
            .input_defaults(command_input_defaults! {
                input: todo_create::TodoCreateInput;
                default input.todo_id = uuid_v7();
            })
            .emits(distributed::events![TodoCreatedDomainEvent])
            .applies(distributed::state_preview! {
                TodoCreatedDomainEvent => todo_domain::TodoState {
                    todo_id: generated.todo_id,
                    owner_id: trusted("x-user-id", "string"),
                    title: input.title,
                    status: "open",
                    assignee_id: null,
                }
            }),
        )
        .handle(todo_create::handle)
        .typed_command(
            typed_command::<todo_rename::TodoRenameInput, Eventual<todo_rename::TodoRenamePayload>>(
                todo_rename::COMMAND,
            )
            .field_name("todos_rename")
            .roles(app_roles)
            .emits(distributed::events![TodoRenamedDomainEvent])
            .applies(distributed::state_preview! {
                TodoRenamedDomainEvent => todo_domain::TodoState {
                    todo_id: input.todo_id,
                    title: input.title,
                    ..unknown
                }
            }),
        )
        .handle(todo_rename::handle)
        .typed_command(
            typed_command::<todo_complete::TodoCompleteInput, Eventual<payloads::TodoStatusPayload>>(
                todo_complete::COMMAND,
            )
            .field_name("todos_complete")
            .roles(app_roles)
            .emits(distributed::events![TodoCompletedDomainEvent])
            .applies(distributed::state_preview! {
                TodoCompletedDomainEvent => todo_domain::TodoState {
                    todo_id: input.todo_id,
                    status: "completed",
                    ..unknown
                }
            }),
        )
        .handle(todo_complete::handle)
        .typed_command(
            typed_command::<todo_reopen::TodoReopenInput, Eventual<todo_reopen::TodoReopenPayload>>(
                todo_reopen::COMMAND,
            )
            .field_name("todos_reopen")
            .roles(app_roles)
            .emits(distributed::events![TodoReopenedDomainEvent])
            .applies(distributed::state_preview! {
                TodoReopenedDomainEvent => todo_domain::TodoState {
                    todo_id: input.todo_id,
                    status: "open",
                    ..unknown
                }
            }),
        )
        .handle(todo_reopen::handle)
        .typed_command(
            typed_command::<todo_archive::TodoArchiveInput, Eventual<todo_archive::TodoArchivePayload>>(
                todo_archive::COMMAND,
            )
            .field_name("todos_archive")
            .roles(app_roles)
            .emits(distributed::events![TodoArchivedDomainEvent])
            .applies(distributed::state_preview! {
                TodoArchivedDomainEvent => todo_domain::TodoState {
                    todo_id: input.todo_id,
                    status: "archived",
                    ..unknown
                }
            }),
        )
        .handle(todo_archive::handle)
        .typed_command(
            typed_command::<
                todo_force_archive::TodoForceArchiveInput,
                Eventual<todo_force_archive::TodoForceArchivePayload>,
            >(todo_force_archive::COMMAND)
            .field_name("todos_force_archive")
            .roles(["admin"])
            .emits(distributed::events![TodoForceArchivedDomainEvent])
            .applies(distributed::state_preview! {
                TodoForceArchivedDomainEvent => todo_domain::TodoState {
                    todo_id: input.todo_id,
                    status: "archived",
                    ..unknown
                }
            }),
        )
        .handle(todo_force_archive::handle)
        .typed_command(
            typed_command::<todo_purge::TodoPurgeInput, Eventual<todo_purge::TodoPurgePayload>>(todo_purge::COMMAND)
                .field_name("todos_purge")
                .roles(app_roles)
                .emits(distributed::events![TodoPurgedDomainEvent])
                .applies(
                    CommandProjectionPreview::new()
                        .events(distributed::events![TodoPurgedDomainEvent])
                        .envelope(
                            ProjectionEnvelopeField::AggregateId,
                            CommandProjectionPreviewSource::input(["todo_id"]),
                        ),
                ),
        )
        .handle(todo_purge::handle)
        .modeled_projector(projections.todo.clone())
        .handle(handlers::events::project_todos::handle);

    let chat = Routes::new()
        .with_repo(
            repo.clone()
                .queued_with(locks.clone())
                .aggregate::<ChatMessage>(),
        )
        .with_read_model_store(read_models.clone())
        .typed_command(
            typed_command::<chat_post::ChatPostInput, Eventual<chat_post::ChatPostPayload>>(
                chat_post::COMMAND,
            )
            .field_name("chat_messages_post")
            .roles(app_roles)
            .emits(distributed::events![ChatMessagePostedDomainEvent])
            .applies(distributed::state_preview! {
                ChatMessagePostedDomainEvent => chat_domain::ChatMessageState {
                    message_id: input.message_id,
                    room_id: input.room_id,
                    author_id: trusted("x-user-id", "string"),
                    body: input.body,
                    created_at: input.created_at,
                }
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
        .modeled_projector(projections.chat.clone())
        .handle(handlers::events::project_chat_messages::handle)
        .events(handlers::events::project_auth_user::EVENTS)
        .guarded(
            handlers::events::project_auth_user::guard,
            handlers::events::project_auth_user::handle,
        );

    let blob = Routes::new()
        .with_repo(repo.queued_with(locks).aggregate::<BlobGame>())
        .with_read_model_store(read_models)
        .typed_command(
            typed_command::<blob_start::BlobStartInput, Atomic<BlobGames>>(blob_start::COMMAND)
                .field_name("blob_games_start")
                .roles(app_roles)
                .emits(distributed::events![BlobStartedDomainEvent])
                // Same mutation IR as eventual. Projected waits for the handler
                // row and returns it (confirmDirectProjection). `.applies` is
                // optional pre-network shell — map_json is RNG server-side.
                .applies(distributed::state_preview! {
                    BlobStartedDomainEvent => blob_domain::BlobGameState {
                        game_id: input.game_id,
                        owner_id: trusted("x-user-id", "string"),
                        score: 0,
                        player_dead: unknown,
                        current_level: 1,
                        current_level_completed: unknown,
                        map_json: "[]",
                        status: "active",
                    }
                }),
        )
        .handle(blob_start::handle)
        .typed_command(
            typed_command::<blob_move::BlobMoveInput, Atomic<BlobGames>>(blob_move::COMMAND)
                .field_name("blob_games_move")
                .roles(app_roles)
                .emits(distributed::events![BlobMovedDomainEvent])
                // Same client path as todos/chat: `.applies` maps command input
                // into the optimistic layer. Client fills board fields from the
                // pure simulate_move twin; server recomputes via domain.
                .applies(distributed::state_preview! {
                    BlobMovedDomainEvent => blob_domain::BlobGameState {
                        game_id: input.game_id,
                        owner_id: trusted("x-user-id", "string"),
                        score: input.score,
                        player_dead: input.player_dead,
                        current_level: input.current_level,
                        current_level_completed: input.current_level_completed,
                        map_json: input.map_json,
                        status: input.status,
                    }
                }),
        )
        .handle(blob_move::handle)
        .typed_command(
            typed_command::<blob_start_level::BlobStartLevelInput, Atomic<BlobGames>>(
                blob_start_level::COMMAND,
            )
            .field_name("blob_games_start_level")
            .roles(app_roles)
            .emits(distributed::events![BlobLevelStartedDomainEvent])
            .applies(distributed::state_preview! {
                BlobLevelStartedDomainEvent => blob_domain::BlobGameState {
                    game_id: input.game_id,
                    owner_id: trusted("x-user-id", "string"),
                    score: unknown,
                    player_dead: unknown,
                    current_level: unknown,
                    current_level_completed: unknown,
                    map_json: "[]",
                    status: "active",
                }
            }),
        )
        .handle(blob_start_level::handle);

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
    build_graphql_engine_with_graphiql(pool, service, identity, change_rx, graphiql_enabled())
}

fn build_graphql_engine_with_graphiql(
    pool: impl Into<GraphqlPoolSource>,
    service: &Service,
    identity: IdentityConfig,
    change_rx: Option<tokio::sync::broadcast::Receiver<distributed::ReadModelChange>>,
    graphiql: bool,
) -> Result<GraphqlEngine, String> {
    let projections = projection_owners();
    let mut b = GraphqlEngine::builder(pool)
        .protocol_token_key(E2E_PROTOCOL_TOKEN_KEY)
        .roles(&["user", "admin", "anonymous"])
        // e2e-ui: eligible admin+user (multi-role principals may open); schema
        // privilege remains user-only so owner-scoped models keep portable row
        // policies for optimistic list inserts (see distributed_client_surface).
        // e2e-ui-admin: elevated ops / all-rows views (/admin).
        // e2e-ui-public: unauthenticated lobby read (anonymous privilege).
        .client_application_surface_with_schema_roles(
            DISTRIBUTED_CLIENT_SURFACE,
            ["admin", "user"],
            ["user"],
        )
        .client_application_surface(DISTRIBUTED_ADMIN_CLIENT_SURFACE, ["admin"])
        .client_application_surface(DISTRIBUTED_PUBLIC_CLIENT_SURFACE, ["anonymous"])
        // user: only own rows. admin: all owners (UI: /admin all-notes view).
        .model::<Todos>(Todos::permissions())
        .model::<ChatMessages>(ChatMessages::permissions())
        .model::<BlobGames>(BlobGames::permissions())
        // Imported IdP directory (join target for todo/blob owner and chat author).
        // Readable by all authenticated roles; writes only via Zitadel projector.
        .model::<AuthUsers>(AuthUsers::permissions())
        .service(service)
        .client_projection_owners([
            projections.todo.into(),
            projections.chat.into(),
            projections.blob.into(),
        ])
        .identity(identity)
        // GraphiQL is a local template convenience. Disable with GRAPHIQL=0
        // (never ship a public edge with GraphiQL + DevHeaders).
        .graphiql(graphiql);
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
    // Allow empty identity so e2e-ui-public (anonymous) can open without a Bearer.
    // Invalid/malformed tokens still 401.
    oidc.require_auth = false;
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

    #[test]
    fn application_todos_keep_portable_owner_row_policy_for_optimistic_list_inserts() {
        use distributed::graphql::ClientRowPolicy;

        let manifest = distributed_client_surface().manifest().unwrap();
        let todos = manifest
            .models
            .iter()
            .find(|model| model.typename == "Todos")
            .expect("Todos model on application surface");
        match &todos.row_policy {
            ClientRowPolicy::Predicate { expression } => {
                let text = serde_json::to_string(expression).expect("serialize row policy");
                assert!(
                    text.contains("x-user-id") && text.contains("owner_id"),
                    "owner claim predicate must be client-portable: {text}"
                );
            }
            other => panic!(
                "Todos must not collapse to server-only row policy (blocks optimistic create list membership); got {other:?}"
            ),
        }

        let blob = manifest
            .models
            .iter()
            .find(|model| model.typename == "BlobGames")
            .expect("BlobGames model on application surface");
        assert!(
            matches!(blob.row_policy, ClientRowPolicy::Predicate { .. }),
            "BlobGames should keep portable owner row policy"
        );
    }

    #[test]
    fn chat_manifest_uses_unit_partition_so_lobby_live_can_stay_active() {
        let manifest = distributed_client_surface().manifest().unwrap();
        let program = manifest
            .projection_programs
            .iter()
            .find(|program| program.name == "project_chat_messages")
            .expect("Chat projection program should be exported");
        assert!(
            program.arms.iter().all(|arm| matches!(
                &arm.partition,
                distributed::graphql::ClientProjectionPartition::Unit
            )),
            "lobby chat uses unit partition so the chat_messages live query can advertise \
             supported index evidence (room isolation stays in the GraphQL where clause). \
             Surface-wide live_resume may still be false when owner-scoped models share the surface."
        );
    }

    #[test]
    fn blob_projection_owner_has_no_async_fact_route() {
        let manifest = distributed_client_surface().manifest().unwrap();
        let owner = manifest
            .projectors
            .iter()
            .find(|projector| projector.name == "project_blob")
            .expect("Blob direct owner should be exported");
        assert!(owner.facts.is_empty());
        assert!(!owner.causal_confirmation);

        let repository = InMemoryRepository::new();
        let service = build_service(
            repository.clone(),
            ClientSurfaceLocks::default(),
            repository,
        );
        let plan = service.subscription_plan();
        for event in [
            "todo.created",
            "todo.renamed",
            "todo.completed",
            "todo.reopened",
            "todo.archived",
            "todo.force_archived",
            "todo.purged",
            "chat_message.posted",
        ] {
            assert!(
                plan.events.iter().any(|candidate| candidate == event),
                "eventual modeled projection must subscribe to {event}"
            );
        }
        for fact in [
            "blob.started",
            "blob.initialized",
            "blob.level_started",
            "blob.moved",
        ] {
            assert!(
                !plan.events.iter().any(|event| event == fact),
                "direct-only Blob ownership must not register an async route for {fact}"
            );
        }
    }

    #[tokio::test]
    async fn graphiql_does_not_change_the_postgres_runtime_client_manifest() {
        let generated = distributed_client_surface().manifest().unwrap();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@localhost/distributed")
            .unwrap();
        let repository = distributed::PostgresRepository::new(pool.clone());
        let service = build_service(
            repository.clone(),
            distributed::PostgresLockManager::new(pool),
            repository.clone(),
        );
        let engine =
            build_graphql_engine_with_graphiql(&repository, &service, dev_identity(), None, true)
                .expect("engine");
        let runtime = engine
            .client_manifest_for_application(DISTRIBUTED_CLIENT_SURFACE, &["admin", "user"])
            .unwrap();

        assert_eq!(generated, runtime);

        let make_request = || {
            serde_json::from_value(serde_json::json!({
                "query": "{ todos @skip(if: true) { todo_id } }",
                "extensions": {
                    "distributed": {
                        "client": {
                            "surface": {
                                "kind": "application",
                                "name": DISTRIBUTED_CLIENT_SURFACE,
                                "roles": ["admin", "user"]
                            },
                            "schemaHash": generated.schema_fingerprint
                        }
                    }
                }
            }))
            .expect("generated application request")
        };
        let mut session = distributed::microsvc::Session::new();
        session.set("x-roles", "user");
        session.set("x-user-id", "person-1");
        let response = engine.execute(&session, make_request()).await;
        assert!(
            !response.is_err(),
            "the runtime must accept the generated application surface: {:?}",
            response.errors
        );
        // Multi-role admin principal may open the same portable contract.
        let mut admin = session.clone();
        admin.set("x-roles", "admin,user");
        let admin_response = engine.execute(&admin, make_request()).await;
        assert!(
            !admin_response.is_err(),
            "admin with user asserted roles must open e2e-ui: {:?}",
            admin_response.errors
        );
        let envelope = response
            .extensions
            .get("distributed")
            .expect("distributed protocol envelope");
        let envelope = serde_json::to_value(envelope).expect("serialized protocol envelope");
        assert_eq!(
            envelope["schemaHash"], generated.schema_fingerprint,
            "the authoritative response must attest the generated schema"
        );
    }

    /// Empty-session open of e2e-ui-public + chat query (anonymous privilege).
    ///
    /// Bare protocol path for unauthenticated lobby peeks; UI route `/public`
    /// documents the same surface name and extension shape.
    #[tokio::test]
    async fn public_surface_opens_and_queries_chat_without_identity() {
        let generated = distributed_public_client_surface().manifest().unwrap();
        assert_eq!(
            generated.surface,
            distributed::graphql::ClientSurfaceIdentity::application(
                DISTRIBUTED_PUBLIC_CLIENT_SURFACE,
                ["anonymous"],
            )
        );
        let repository = distributed::SqliteRepository::connect_and_migrate("sqlite::memory:")
            .await
            .expect("sqlite memory repo");
        let registry = e2e_readmodels::distributed_manifest()
            .table_registry()
            .expect("registry");
        repository
            .bootstrap_table_schema_for_dev(&registry)
            .await
            .expect("bootstrap tables");
        let service = build_service(
            repository.clone(),
            ClientSurfaceLocks::default(),
            repository.clone(),
        );
        let engine =
            build_graphql_engine_with_graphiql(&repository, &service, dev_identity(), None, false)
                .expect("engine");
        let runtime = engine
            .client_manifest_for_application(DISTRIBUTED_PUBLIC_CLIENT_SURFACE, &["anonymous"])
            .expect("public surface registered");
        assert_eq!(generated.schema_fingerprint, runtime.schema_fingerprint);

        let request = serde_json::from_value(serde_json::json!({
            "query": "{ chat_messages(limit: 5, offset: 0) { message_id body room_id } }",
            "extensions": {
                "distributed": {
                    "client": {
                        "surface": {
                            "kind": "application",
                            "name": DISTRIBUTED_PUBLIC_CLIENT_SURFACE,
                            "roles": ["anonymous"]
                        },
                        "schemaHash": generated.schema_fingerprint
                    }
                }
            }
        }))
        .expect("public application request");

        // No x-user-id, no x-roles — unauthenticated principal.
        let session = distributed::microsvc::Session::new();
        let response = engine.execute(&session, request).await;
        assert!(
            !response.is_err(),
            "anonymous open + chat query must succeed: {:?}",
            response.errors
        );
        let data = response.data.into_json().expect("json data");
        assert!(
            data.get("chat_messages").and_then(|v| v.as_array()).is_some(),
            "expected chat_messages array: {data}"
        );
        let envelope = response
            .extensions
            .get("distributed")
            .expect("distributed protocol envelope");
        let envelope = serde_json::to_value(envelope).expect("serialized protocol envelope");
        assert_eq!(envelope["schemaHash"], generated.schema_fingerprint);
    }
}
