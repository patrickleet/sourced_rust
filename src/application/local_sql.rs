//! Framework-owned local SQL realization for one process plan.
//!
//! Connects the dialect, binds only capabilities that were actually started,
//! constructs the dispatcher, starts workers required by the plan, and
//! exposes a GraphQL router through [`CommandDispatcher`]. Application code
//! supplies module compose + surface compilation; it must not open a dialect
//! runner.

use std::sync::Arc;
use std::time::Duration;

use super::capability::Capability;
use super::error::{ApplicationError, ApplicationResult};
use super::plan::DeploymentPlan;
use super::runtime_host::{CapabilityProviders, RuntimeHost};
use crate::command_dispatch::{LocalCommandDispatcher, SharedCommandDispatcher};
use crate::graphql::{graphql_router_with_dispatcher, GraphqlEngine, IdentityConfig};
use crate::microsvc::{spawn_outbox_publish_loop, spawn_service_consumer_loop, Service};
use crate::table::TableSchemaRegistry;

/// Options for realizing one local SQL process.
pub struct LocalSqlOptions {
    pub database_url: String,
    pub identity: IdentityConfig,
    pub bus_group: String,
    pub registry: TableSchemaRegistry,
}

/// Live SQL handles after the framework has connected and bootstrapped.
pub enum LocalSqlHandles {
    #[cfg(feature = "sqlite")]
    Sqlite {
        repo: crate::SqliteRepository,
        locks: crate::SqliteLockManager,
        bus: crate::bus::SqliteBus,
    },
    #[cfg(feature = "postgres")]
    Postgres {
        repo: crate::PostgresRepository,
        locks: crate::PostgresLockManager,
        bus: crate::bus::PostgresBus,
    },
}

/// Application hooks invoked after the framework has started SQL providers.
pub trait LocalSqlApplication: Clone + Send + Sync + 'static {
    fn compose(&self, handles: &LocalSqlHandles) -> Service;
    fn engine(
        &self,
        handles: &LocalSqlHandles,
        service: &Service,
        identity: IdentityConfig,
        change_rx: Option<tokio::sync::broadcast::Receiver<crate::ReadModelChange>>,
    ) -> Result<GraphqlEngine, String>;
    fn after_bound(&self, handles: &LocalSqlHandles) {
        let _ = handles;
    }
}

/// A bound process with live dispatcher, engine, and service.
pub struct RealizedRuntimeHost {
    pub host: RuntimeHost,
    pub dispatcher: SharedCommandDispatcher,
    pub engine: Arc<GraphqlEngine>,
    pub service: Arc<Service>,
    pub identity: IdentityConfig,
}

impl RealizedRuntimeHost {
    /// GraphQL + health router dispatched through [`CommandDispatcher`].
    pub fn graphql_router(&self) -> axum::Router {
        graphql_router_with_dispatcher(Arc::clone(&self.engine), Arc::clone(&self.dispatcher))
            .route(
                "/health",
                axum::routing::get(|| async { axum::Json(serde_json::json!({ "ok": true })) }),
            )
    }

    /// Serve an already-decorated router (application may add identity/ingress).
    pub async fn serve_router(
        self,
        bind: &str,
        router: axum::Router,
    ) -> Result<(), std::io::Error> {
        let listener = tokio::net::TcpListener::bind(bind).await?;
        axum::serve(listener, router).await
    }
}

impl RuntimeHost {
    /// Realize one process: connect SQL providers, bind the plan, start workers.
    ///
    /// Only capabilities actually started are presented to [`RuntimeHost::bind`].
    /// Missing plan requirements fail closed before serve.
    pub async fn realize_local(
        plan: &DeploymentPlan,
        process_id: &str,
        options: LocalSqlOptions,
        app: impl LocalSqlApplication,
    ) -> ApplicationResult<RealizedRuntimeHost> {
        let url = options.database_url.as_str();
        if url.starts_with("postgres://") || url.starts_with("postgresql://") {
            #[cfg(feature = "postgres")]
            {
                return realize_postgres(plan, process_id, options, app).await;
            }
            #[cfg(not(feature = "postgres"))]
            {
                return Err(ApplicationError::InvalidSpec(
                    "postgres URL provided but the postgres feature is not enabled".into(),
                ));
            }
        }
        #[cfg(feature = "sqlite")]
        {
            realize_sqlite(plan, process_id, options, app).await
        }
        #[cfg(not(feature = "sqlite"))]
        {
            Err(ApplicationError::InvalidSpec(
                "sqlite URL provided but the sqlite feature is not enabled".into(),
            ))
        }
    }
}

fn sql_local_providers() -> CapabilityProviders {
    CapabilityProviders::default()
        .with(Capability::EventStore)
        .with(Capability::LockManager)
        .with(Capability::CommandLedger)
        .with(Capability::TransactionalOutbox)
        .with(Capability::Publisher)
        .with(Capability::EventSubscription)
        .with(Capability::InboxCheckpoint)
        .with(Capability::ReadStore)
        .with(Capability::ChangeFeed)
        .with(Capability::SchemaLifecycle)
        .with(Capability::LocalCommandDispatch)
        .with(Capability::IdentityMiddleware)
        .with(Capability::HttpTransport)
        .with(Capability::WebsocketTransport)
        .with(Capability::Metrics)
        .with(Capability::DirectProjectionTransaction)
}

fn process_requires(plan: &DeploymentPlan, process_id: &str, capability: Capability) -> bool {
    plan.processes
        .iter()
        .find(|process| process.id == process_id)
        .map(|process| {
            process
                .capabilities
                .iter()
                .any(|requirement| requirement.capability == capability)
        })
        .unwrap_or(false)
}

fn bind_realized(
    plan: &DeploymentPlan,
    process_id: &str,
    service: Service,
    engine: GraphqlEngine,
    identity: IdentityConfig,
) -> ApplicationResult<RealizedRuntimeHost> {
    let service = Arc::new(service);
    let dispatcher: SharedCommandDispatcher =
        Arc::new(LocalCommandDispatcher::new(Arc::clone(&service)));
    let host = RuntimeHost::bind(
        plan,
        process_id,
        sql_local_providers(),
        Some(Arc::clone(&dispatcher)),
    )?;
    Ok(RealizedRuntimeHost {
        host,
        dispatcher,
        engine: Arc::new(engine),
        service,
        identity,
    })
}

#[cfg(feature = "sqlite")]
async fn realize_sqlite(
    plan: &DeploymentPlan,
    process_id: &str,
    options: LocalSqlOptions,
    app: impl LocalSqlApplication,
) -> ApplicationResult<RealizedRuntimeHost> {
    let repo = crate::SqliteRepository::connect_and_migrate(&options.database_url)
        .await
        .map_err(|error| ApplicationError::InvalidSpec(format!("sqlite connect: {error}")))?;
    repo.bootstrap_table_schema_for_dev(&options.registry)
        .await
        .map_err(|error| ApplicationError::InvalidSpec(format!("sqlite schema: {error}")))?;
    let locks = crate::SqliteLockManager::new(repo.pool().clone());
    let bus = crate::bus::SqliteBus::new(repo.pool().clone()).group(options.bus_group.clone());
    bus.ensure_tables()
        .await
        .map_err(|error| ApplicationError::InvalidSpec(format!("sqlite bus: {error}")))?;
    let change_rx = repo.read_model_changes();
    let handles = LocalSqlHandles::Sqlite {
        repo: repo.clone(),
        locks: locks.clone(),
        bus: crate::bus::SqliteBus::new(repo.pool().clone()).group(options.bus_group.clone()),
    };
    let service = app.compose(&handles).with_bus(
        crate::bus::SqliteBus::new(repo.pool().clone()).group(options.bus_group.clone()),
    );
    let engine = app
        .engine(&handles, &service, options.identity.clone(), Some(change_rx))
        .map_err(ApplicationError::InvalidSpec)?;
    if process_requires(plan, process_id, Capability::TransactionalOutbox)
        || process_requires(plan, process_id, Capability::Publisher)
    {
        spawn_outbox_publish_loop(
            repo.outbox_store(),
            Arc::new(crate::bus::SqliteBus::new(repo.pool().clone()).group(options.bus_group.clone())),
            plan.application.clone(),
            Duration::from_secs(30),
            5,
        );
    }
    app.after_bound(&handles);
    if process_requires(plan, process_id, Capability::EventSubscription)
        || process_requires(plan, process_id, Capability::InboxCheckpoint)
    {
        let group = options.bus_group.clone();
        let app = app.clone();
        spawn_service_consumer_loop(move || {
            let handles = LocalSqlHandles::Sqlite {
                repo: repo.clone(),
                locks: locks.clone(),
                bus: crate::bus::SqliteBus::new(repo.pool().clone()).group(group.clone()),
            };
            app.compose(&handles).with_bus(
                crate::bus::SqliteBus::new(repo.pool().clone()).group(group.clone()),
            )
        });
    }
    bind_realized(plan, process_id, service, engine, options.identity)
}

#[cfg(feature = "postgres")]
async fn realize_postgres(
    plan: &DeploymentPlan,
    process_id: &str,
    options: LocalSqlOptions,
    app: impl LocalSqlApplication,
) -> ApplicationResult<RealizedRuntimeHost> {
    let repo = crate::PostgresRepository::connect_and_migrate(&options.database_url)
        .await
        .map_err(|error| ApplicationError::InvalidSpec(format!("postgres connect: {error}")))?;
    repo.bootstrap_table_schema_for_dev(&options.registry)
        .await
        .map_err(|error| ApplicationError::InvalidSpec(format!("postgres schema: {error}")))?;
    let locks = crate::PostgresLockManager::new(repo.pool().clone());
    let bus = crate::bus::PostgresBus::new(repo.pool().clone()).group(options.bus_group.clone());
    bus.ensure_tables()
        .await
        .map_err(|error| ApplicationError::InvalidSpec(format!("postgres bus: {error}")))?;
    let change_rx = repo.read_model_changes();
    let handles = LocalSqlHandles::Postgres {
        repo: repo.clone(),
        locks: locks.clone(),
        bus: crate::bus::PostgresBus::new(repo.pool().clone()).group(options.bus_group.clone()),
    };
    let service = app.compose(&handles).with_bus(
        crate::bus::PostgresBus::new(repo.pool().clone()).group(options.bus_group.clone()),
    );
    let engine = app
        .engine(&handles, &service, options.identity.clone(), Some(change_rx))
        .map_err(ApplicationError::InvalidSpec)?;
    if process_requires(plan, process_id, Capability::TransactionalOutbox)
        || process_requires(plan, process_id, Capability::Publisher)
    {
        spawn_outbox_publish_loop(
            repo.outbox_store(),
            Arc::new(
                crate::bus::PostgresBus::new(repo.pool().clone()).group(options.bus_group.clone()),
            ),
            plan.application.clone(),
            Duration::from_secs(30),
            5,
        );
    }
    app.after_bound(&handles);
    if process_requires(plan, process_id, Capability::EventSubscription)
        || process_requires(plan, process_id, Capability::InboxCheckpoint)
    {
        let group = options.bus_group.clone();
        let app = app.clone();
        spawn_service_consumer_loop(move || {
            let handles = LocalSqlHandles::Postgres {
                repo: repo.clone(),
                locks: locks.clone(),
                bus: crate::bus::PostgresBus::new(repo.pool().clone()).group(group.clone()),
            };
            app.compose(&handles).with_bus(
                crate::bus::PostgresBus::new(repo.pool().clone()).group(group.clone()),
            )
        });
    }
    bind_realized(plan, process_id, service, engine, options.identity)
}

#[cfg(all(test, feature = "sqlite", feature = "graphql"))]
mod tests {
    use super::*;
    use crate::application::{
        compile_deployment_plan, Application, CommandDefinition, CommandSpec, CommandTypeSpec,
        ModelFieldSpec, ModelSpec, Module, ProcessIntent, ProcessPreset, ProjectionSpec,
    };
    use crate::graphql::CommandConsistency;

    #[derive(Clone)]
    struct TestApp;

    impl LocalSqlApplication for TestApp {
        fn compose(&self, _handles: &LocalSqlHandles) -> Service {
            Service::new().named("local-sql-test")
        }

        fn engine(
            &self,
            handles: &LocalSqlHandles,
            _service: &Service,
            identity: IdentityConfig,
            _change_rx: Option<tokio::sync::broadcast::Receiver<crate::ReadModelChange>>,
        ) -> Result<GraphqlEngine, String> {
            let repo = match handles {
                LocalSqlHandles::Sqlite { repo, .. } => repo,
                #[cfg(feature = "postgres")]
                LocalSqlHandles::Postgres { .. } => {
                    return Err("test app expects sqlite".into());
                }
            };
            GraphqlEngine::builder(repo)
                .roles(&["user"])
                .identity(identity)
                .graphiql(false)
                .build()
                .map_err(|error| error.to_string())
        }
    }

    fn sample_plan() -> DeploymentPlan {
        let model = ModelSpec::try_new(
            "TodoView",
            "todos",
            [ModelFieldSpec {
                name: "todo_id".into(),
                scalar: "String".into(),
                nullable: false,
            }],
            ["todo_id"],
        )
        .unwrap();
        let command = CommandSpec::try_new(
            "todo.create",
            "todo_create",
            CommandTypeSpec {
                name: "In".into(),
                fields: vec![],
            },
            CommandTypeSpec {
                name: "Out".into(),
                fields: vec![],
            },
            CommandConsistency::Eventual,
        )
        .unwrap();
        let module = Module::new("todo")
            .command_definitions([CommandDefinition::contract(command)])
            .models([model])
            .projections([
                ProjectionSpec::try_new("project_todos", ["todo.created"], ["TodoView"]).unwrap(),
            ])
            .build()
            .unwrap();
        let manifest = Application::new("todo-app")
            .module(module)
            .build()
            .unwrap()
            .manifest()
            .clone();
        compile_deployment_plan(
            "local",
            &manifest,
            [ProcessIntent::with_preset("full", &manifest, ProcessPreset::Commands).unwrap()],
        )
        .unwrap()
    }

    #[tokio::test]
    async fn realize_local_starts_sqlite_providers_and_binds() {
        let plan = sample_plan();
        let realized = RuntimeHost::realize_local(
            &plan,
            "full",
            LocalSqlOptions {
                database_url: "sqlite::memory:".into(),
                identity: IdentityConfig::dev_headers(),
                bus_group: "test".into(),
                registry: TableSchemaRegistry::new(),
            },
            TestApp,
        )
        .await
        .expect("realize sqlite");
        assert_eq!(realized.host.process_id(), "full");
        assert!(realized.host.dispatcher().is_some());
        assert!(realized.host.providers.contains(Capability::EventStore));
        assert!(realized.host.providers.contains(Capability::LockManager));
        assert!(realized
            .host
            .providers
            .contains(Capability::LocalCommandDispatch));
        assert!(!realized
            .host
            .providers
            .contains(Capability::RemoteCommandDispatch));
    }
}
