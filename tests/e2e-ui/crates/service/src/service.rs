//! Route bundles + GraphQL engine for the e2e-ui fixture.

use std::collections::BTreeMap;
use std::sync::Arc;

use blob_domain::{
    BlobGame, BlobLevelStartedDomainEvent, BlobMovedDomainEvent, BlobStartedDomainEvent,
};
use chat_domain::{ChatMessage, ChatMessagePostedDomainEvent};
use distributed::graphql::{
    build_surface, read, surface_for_application, typed_command, DistributedClientSurfaceExport,
    Causal, CommandProjectionPreview, CommandProjectionPreviewSource, GraphqlEngine,
    GraphqlPoolSource, IdentityConfig, ModelPermissions, OidcConfig, Projected, RoleGrant,
    SurfaceDirectProjection, SurfaceModeledProjection, SurfaceOptions, SurfaceProjector,
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
use distributed::{
    command_input_defaults, AggregateBuilder, AggregateRepository, InMemoryLockManager,
    InMemoryRepository, LockError, LockManager, ProjectionEnvelopeField, Queueable,
    QueuedRepository, RelationalReadModel,
};
use distributed::projection_protocol::ProjectorTopologyId;
use e2e_readmodels::{
    AuthUsers, BlobGames, ChatMessages, Todos, BLOB_GAMES, CHAT_MESSAGES, TODO_READS,
};
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
    ProjectionOutput::try_new(
        schema.model_name.clone(),
        schema.table_name.clone(),
        schema,
    )
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
        TODO_READS.eventual(),
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
                ProjectionExecutorRoute::local("e2e-ui")
                    .expect("canonical local projection route"),
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

    ProjectionOwners {
        todo: SurfaceProjector::new("project_todos")
            .modeled(modeled_projection(
                TODO_READS,
                &catalog,
                &active,
                &todo_binding,
            )),
        chat: SurfaceProjector::new("project_chat_messages")
            .modeled(modeled_projection(
                CHAT_MESSAGES,
                &catalog,
                &active,
                &chat_binding,
            )),
        blob: SurfaceDirectProjection::new("project_blob")
            .modeled(modeled_projection(
                BLOB_GAMES,
                &catalog,
                &active,
                &blob_binding,
            )),
    }
}

fn client_grants() -> BTreeMap<String, BTreeMap<String, RoleGrant>> {
    let all_models = || {
        BTreeMap::from([
            ("AuthUsers".into(), RoleGrant::all_columns()),
            ("BlobGames".into(), RoleGrant::all_columns()),
            ("ChatMessages".into(), RoleGrant::all_columns()),
            ("Todos".into(), RoleGrant::all_columns()),
        ])
    };
    let mut user = all_models();
    user.insert(
        "Todos".into(),
        RoleGrant::all_columns().rows(
            distributed::graphql::col("owner_id").eq(distributed::graphql::claim("x-user-id")),
        ),
    );
    user.insert(
        "BlobGames".into(),
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
///
/// Includes both `user` and `admin`: an admin is still a person using todos/
/// chat/blob. Elevated-only ops stay on [`distributed_admin_client_surface`].
/// Differing row policies (owner vs all) become `ServerOnly` on the shared
/// surface so the server re-checks membership per concrete role.
pub fn distributed_client_surface() -> DistributedClientSurfaceExport {
    pool_free_client_surface(DISTRIBUTED_CLIENT_SURFACE, &["admin", "user"])
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
        force_archive, payloads, purge, rename, reopen,
    };

    let app_roles = ["user", "admin"];
    let projections = projection_owners();
    let todos = Routes::new()
        .with_repo(repo.clone().queued_with(locks.clone()).aggregate::<Todo>())
        .with_read_model_store(read_models.clone())
        .typed_command(
            typed_command::<create::TodoCreateInput, Causal<create::TodoCreatePayload>>(
                create::COMMAND,
            )
            .field_name("todos_create")
            .roles(app_roles)
            .input_defaults(command_input_defaults! {
                input: create::TodoCreateInput;
                default input.todo_id = uuid_v7();
            })
            .emits(distributed::events![TodoCreatedDomainEvent])
            .preview(distributed::state_preview! {
                TodoCreatedDomainEvent => todo_domain::TodoState {
                    todo_id: generated.todo_id,
                    owner_id: trusted("x-user-id", "string"),
                    title: input.title,
                    status: "open",
                    assignee_id: null,
                }
            }),
        )
        .handle(create::handle)
        .typed_command(
            typed_command::<rename::TodoRenameInput, Causal<rename::TodoRenamePayload>>(
                rename::COMMAND,
            )
            .field_name("todos_rename")
            .roles(app_roles)
            .emits(distributed::events![TodoRenamedDomainEvent])
            .preview(distributed::state_preview! {
                TodoRenamedDomainEvent => todo_domain::TodoState {
                    todo_id: input.todo_id,
                    title: input.title,
                    ..unknown
                }
            }),
        )
        .handle(rename::handle)
        .typed_command(
            typed_command::<complete::TodoCompleteInput, Causal<payloads::TodoStatusPayload>>(
                complete::COMMAND,
            )
            .field_name("todos_complete")
            .roles(app_roles)
            .emits(distributed::events![TodoCompletedDomainEvent])
            .preview(e2e_readmodels::complete_preview()),
        )
        .handle(complete::handle)
        .typed_command(
            typed_command::<reopen::TodoReopenInput, Causal<reopen::TodoReopenPayload>>(
                reopen::COMMAND,
            )
            .field_name("todos_reopen")
            .roles(app_roles)
            .emits(distributed::events![TodoReopenedDomainEvent])
            .preview(distributed::state_preview! {
                TodoReopenedDomainEvent => todo_domain::TodoState {
                    todo_id: input.todo_id,
                    status: "open",
                    ..unknown
                }
            }),
        )
        .handle(reopen::handle)
        .typed_command(
            typed_command::<archive::TodoArchiveInput, Causal<archive::TodoArchivePayload>>(
                archive::COMMAND,
            )
            .field_name("todos_archive")
            .roles(app_roles)
            .emits(distributed::events![TodoArchivedDomainEvent])
            .preview(distributed::state_preview! {
                TodoArchivedDomainEvent => todo_domain::TodoState {
                    todo_id: input.todo_id,
                    status: "archived",
                    ..unknown
                }
            }),
        )
        .handle(archive::handle)
        .typed_command(
            typed_command::<
                force_archive::TodoForceArchiveInput,
                Causal<force_archive::TodoForceArchivePayload>,
            >(force_archive::COMMAND)
            .field_name("todos_force_archive")
            .roles(["admin"])
            .emits(distributed::events![TodoForceArchivedDomainEvent])
            .preview(distributed::state_preview! {
                TodoForceArchivedDomainEvent => todo_domain::TodoState {
                    todo_id: input.todo_id,
                    status: "archived",
                    ..unknown
                }
            }),
        )
        .handle(force_archive::handle)
        .typed_command(
            typed_command::<purge::TodoPurgeInput, Causal<purge::TodoPurgePayload>>(
                purge::COMMAND,
            )
            .field_name("todos_purge")
            .roles(app_roles)
            .emits(distributed::events![TodoPurgedDomainEvent])
            .preview(
                CommandProjectionPreview::new()
                    .events(distributed::events![TodoPurgedDomainEvent])
                    .envelope(
                        ProjectionEnvelopeField::AggregateId,
                        CommandProjectionPreviewSource::input(["todo_id"]),
                    ),
            ),
        )
        .handle(purge::handle)
        .consume_projection(projections.todo.clone());

    let chat = Routes::new()
        .with_repo(
            repo.clone()
                .queued_with(locks.clone())
                .aggregate::<ChatMessage>(),
        )
        .with_read_model_store(read_models.clone())
        .typed_command(
            typed_command::<chat_post::ChatPostInput, Causal<chat_post::ChatPostPayload>>(
                chat_post::COMMAND,
            )
            .field_name("chat_messages_post")
            .roles(app_roles)
            .emits(distributed::events![ChatMessagePostedDomainEvent])
            .preview(distributed::state_preview! {
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
        .consume_projection(projections.chat.clone())
        .events(handlers::events::project_auth_user::EVENTS)
        .guarded(
            handlers::events::project_auth_user::guard,
            handlers::events::project_auth_user::handle,
        );

    let blob = Routes::new()
        .with_repo(repo.queued_with(locks).aggregate::<BlobGame>())
        .with_read_model_store(read_models)
        .typed_command(
            typed_command::<blob_start::BlobStartInput, Projected<BlobGames>>(
                blob_start::COMMAND,
            )
            .field_name("blob_games_start")
            .roles(app_roles)
            .emits(distributed::events![BlobStartedDomainEvent]),
        )
        .handle(blob_start::handle)
        .typed_command(
            typed_command::<blob_move::BlobMoveInput, Projected<BlobGames>>(blob_move::COMMAND)
                .field_name("blob_games_move")
                .roles(app_roles)
                .emits(distributed::events![BlobMovedDomainEvent]),
        )
        .handle(blob_move::handle)
        .typed_command(
            typed_command::<blob_start_level::BlobStartLevelInput, Projected<BlobGames>>(
                blob_start_level::COMMAND,
            )
            .field_name("blob_games_start_level")
            .roles(app_roles)
            .emits(distributed::events![BlobLevelStartedDomainEvent]),
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
        .roles(&["user", "admin"])
        // e2e-ui: everyone who can sign in (admin is a superset of user).
        // e2e-ui-admin: elevated ops only (/admin).
        .client_application_surface(DISTRIBUTED_CLIENT_SURFACE, ["admin", "user"])
        .client_application_surface(DISTRIBUTED_ADMIN_CLIENT_SURFACE, ["admin"])
        // user: only own rows. admin: all owners (UI: /admin all-notes view).
        .model::<Todos>(
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
        .model::<ChatMessages>(
            ModelPermissions::new()
                .grant("user", read().all_columns())
                .grant("admin", read().all_columns()),
        )
        .model::<BlobGames>(
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
        // Imported IdP directory (join target for todo/blob owner and chat author).
        // Readable by all authenticated roles; writes only via Zitadel projector.
        .model::<AuthUsers>(
            ModelPermissions::new()
                .grant("user", read().all_columns())
                .grant("admin", read().all_columns()),
        )
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
    fn chat_manifest_retains_dynamic_room_partition_without_unit_resume_capability() {
        let manifest = distributed_admin_client_surface().manifest().unwrap();
        let program = manifest
            .projection_programs
            .iter()
            .find(|program| program.name == "project_chat_messages")
            .expect("Chat projection program should be exported");
        assert!(
            program.arms.iter().all(|arm| matches!(
                &arm.partition,
                distributed::graphql::ClientProjectionPartition::Expression { .. }
            )),
            "every Chat arm must retain its room-derived partition expression"
        );
        assert!(
            !manifest.capabilities.live_resume,
            "a room-partitioned projection cannot advertise one unit resume cursor"
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
            .client_manifest_for_application(DISTRIBUTED_CLIENT_SURFACE, &["user", "admin"])
            .unwrap();

        assert_eq!(generated, runtime);

        let request = serde_json::from_value(serde_json::json!({
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
        .expect("generated application request");
        let mut session = distributed::microsvc::Session::new();
        session.set("x-role", "user");
        session.set("x-user-id", "person-1");
        let response = engine.execute(&session, request).await;
        assert!(
            !response.is_err(),
            "the runtime must accept the generated application surface: {:?}",
            response.errors
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
}
