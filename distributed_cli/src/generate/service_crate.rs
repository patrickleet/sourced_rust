//! Rust service-crate templates: `Cargo.toml` and the `src/**` sources
//! (lib/main/manifest/service/models/handlers/read_models). Pure — each method
//! returns the file contents as a `String`.

use super::names::{message_owner, rust_string, toml_string, MessageHandler, ModelScaffold};
use super::Scaffold;
use crate::{MetricsTarget, ServiceTransport, StoreTarget};

impl Scaffold {
    pub(super) fn cargo_toml(&self) -> String {
        let distributed_path = toml_string(&self.distributed_dependency_path);
        let features = self
            .distributed_features()
            .into_iter()
            .map(toml_string)
            .collect::<Vec<_>>()
            .join(", ");
        // Keep in lockstep with the `distributed` crate's own axum major: the
        // Knative main.rs passes distributed's `Router` to this axum's `serve`.
        let axum = if self.transport == ServiceTransport::Knative {
            "axum = \"0.8\"\n"
        } else {
            ""
        };

        format!(
            r#"[package]
name = {package_name}
version = "0.1.0"
edition = "2021"

[workspace]

[dependencies]
distributed = {{ path = {distributed_path}, features = [{features}] }}
{axum}serde = {{ version = "1", features = ["derive"] }}
serde_json = "1"
tokio = {{ version = "1", features = ["macros", "net", "rt-multi-thread"] }}
"#,
            package_name = toml_string(&self.names.package_name),
        )
    }

    fn distributed_features(&self) -> Vec<&'static str> {
        let mut features = Vec::new();
        match self.transport {
            ServiceTransport::Http => features.push("http"),
            ServiceTransport::Knative => features.push("http"),
        }
        match self.store {
            StoreTarget::Postgres => features.push("postgres"),
            StoreTarget::Sqlite => features.push("sqlite"),
            StoreTarget::InMemory => {}
        }
        if self.metrics == Some(MetricsTarget::Prometheus) {
            features.push("metrics");
        }
        features
    }

    pub(super) fn lib_rs(&self) -> String {
        let models = if !self.models.is_empty() {
            "pub mod models;\n"
        } else {
            ""
        };
        let read_models = if self.include_read_models {
            "pub mod read_models;\n"
        } else {
            ""
        };
        format!(
            r#"pub mod handlers;
pub mod manifest;
{models}{read_models}pub mod service;

pub use manifest::distributed_manifest;
"#
        )
    }

    pub(super) fn main_rs(&self) -> String {
        match self.transport {
            ServiceTransport::Http => format!(
                r#"#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {{
    let addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".to_string());
    let service = {crate_ident}::service::in_memory();
    distributed::microsvc::serve(service, &addr).await?;
    Ok(())
}}
"#,
                crate_ident = self.names.crate_ident,
            ),
            ServiceTransport::Knative => format!(
                r#"#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {{
    let addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".to_string());
    let service = {crate_ident}::service::in_memory();
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    let app = distributed::microsvc::cloud_events_router(service);
    axum::serve(listener, app).await?;
    Ok(())
}}
"#,
                crate_ident = self.names.crate_ident,
            ),
        }
    }

    pub(super) fn manifest_rs(&self) -> String {
        let read_model_import = if self.include_read_models && !self.read_models.is_empty() {
            format!(
                "use crate::read_models::{{{}}};\n\n",
                self.read_models
                    .iter()
                    .map(|model| model.view_ident.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        } else {
            String::new()
        };
        let read_model_registration = self
            .read_models
            .iter()
            .map(|model| format!("        .read_model::<{}>()\n", model.view_ident))
            .collect::<String>();
        format!(
            r#"use distributed::{{
    DistributedProjectManifest, ServiceManifest,
}};

{read_model_import}pub fn distributed_manifest() -> DistributedProjectManifest {{
    DistributedProjectManifest::new({project_name})
{read_model_registration}        .service(crate::service::manifest())
}}

pub fn service_manifest() -> ServiceManifest {{
    crate::service::manifest()
}}
"#,
            project_name = rust_string(&self.names.package_name),
        )
    }

    pub(super) fn service_rs(&self) -> String {
        let registrations = self
            .commands
            .iter()
            .map(|handler| format!("        command handlers::{},\n", handler.module_ident))
            .chain(
                self.events
                    .iter()
                    .map(|handler| format!("        event handlers::{},\n", handler.module_ident)),
            )
            .collect::<String>();
        let manifest_commands = self
            .commands
            .iter()
            .map(|handler| {
                format!(
                    "        .command(handlers::{}::COMMAND)\n",
                    handler.module_ident
                )
            })
            .collect::<String>();
        let manifest_events = self
            .events
            .iter()
            .map(|handler| {
                format!(
                    "        .event(handlers::{}::EVENT)\n",
                    handler.module_ident
                )
            })
            .collect::<String>();
        let transport = match self.transport {
            ServiceTransport::Http => "http",
            ServiceTransport::Knative => "knative",
        };

        format!(
            r#"use std::sync::Arc;

use distributed::{{
    microsvc::{{Routes, Service}},
    InMemoryRepository, ServiceManifest,
}};

use crate::handlers;

pub type ServiceRepo = InMemoryRepository;

pub fn in_memory() -> Arc<Service> {{
    build(InMemoryRepository::new())
}}

pub fn build(repo: ServiceRepo) -> Arc<Service> {{
    let routes = distributed::routes!(
        Routes::new().with_dependencies(repo),
{registrations}    );
    Arc::new(Service::new().named({service_name}).routes(routes))
}}

pub fn manifest() -> ServiceManifest {{
    ServiceManifest::new({service_name})
{manifest_commands}{manifest_events}        .transport({transport})
}}
"#,
            service_name = rust_string(&self.names.package_name),
            transport = rust_string(transport),
        )
    }

    pub(super) fn models_mod_rs(&self) -> String {
        let modules = self
            .models
            .iter()
            .map(|model| {
                format!(
                    "pub mod {module_ident};\npub use {module_ident}::{type_ident};\n",
                    module_ident = model.module_ident,
                    type_ident = model.type_ident,
                )
            })
            .collect::<Vec<_>>()
            .join("");

        format!(
            r#"{modules}
use serde::{{Deserialize, Serialize}};


#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommandInput {{
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
}}
"#
        )
    }

    pub(super) fn model_rs(&self, model: &ModelScaffold) -> String {
        format!(
            r#"use distributed::{{sourced, Entity, Snapshot}};

#[derive(Default, Snapshot)]
pub struct {model_struct} {{
    pub entity: Entity,
    pub name: Option<String>,
    pub status: String,
}}

#[sourced(entity)]
impl {model_struct} {{
    #[event({command_recorded_event})]
    pub fn record_command(&mut self, command: String, id: String, name: Option<String>) {{
        self.entity.set_id(&id);
        if let Some(name) = name {{
            self.name = Some(name);
        }}
        self.status = command;
    }}
}}
"#,
            model_struct = model.type_ident,
            command_recorded_event =
                rust_string(&format!("{}.command_recorded", model.message_prefix)),
        )
    }

    pub(super) fn handlers_mod_rs(&self) -> String {
        self.commands
            .iter()
            .chain(self.events.iter())
            .map(|handler| format!("pub mod {};\n", handler.module_ident))
            .collect()
    }

    pub(super) fn command_handler_rs(&self, handler: &MessageHandler) -> String {
        if let Some(model) = self.command_model(handler) {
            format!(
                r#"use distributed::{{
    microsvc::{{Context, HandlerError}}, Aggregate, CommitBatch, StreamIdentity, StreamWrite,
    TransactionalCommit,
}};
use serde_json::{{json, Value}};

use crate::models::{{CommandInput, {model_type}}};
use crate::service::ServiceRepo;

pub const COMMAND: &str = {message_name};
pub const MODEL: &str = {model_name};

pub fn guard(ctx: &Context<ServiceRepo>) -> bool {{
    ctx.has_fields(&["id"])
}}

pub async fn handle(ctx: &Context<'_, ServiceRepo>) -> Result<Value, HandlerError> {{
    let input = ctx.input::<CommandInput>()?;
    let mut aggregate = {model_type}::default();
    aggregate.record_command(COMMAND.to_string(), input.id.clone(), input.name.clone())?;
    let identity = StreamIdentity::new({model_type}::aggregate_type(), aggregate.entity().id())?;
    let stream = StreamWrite::new(identity, aggregate.entity_mut());
    ctx.repo().commit_batch(CommitBatch::new(vec![stream])).await?;
    Ok(json!({{ "command": COMMAND, "id": input.id, "model": MODEL, "name": input.name }}))
}}
"#,
                model_type = model.type_ident,
                message_name = rust_string(&handler.message_name),
                model_name = rust_string(&model.name),
            )
        } else {
            format!(
                r#"use distributed::microsvc::{{Context, HandlerError}};
use serde_json::{{json, Value}};

use crate::service::ServiceRepo;

pub const COMMAND: &str = {message_name};

pub fn guard(_ctx: &Context<ServiceRepo>) -> bool {{
    true
}}

pub async fn handle(ctx: &Context<'_, ServiceRepo>) -> Result<Value, HandlerError> {{
    let input = ctx.input::<Value>()?;
    Ok(json!({{ "command": COMMAND, "input": input }}))
}}
"#,
                message_name = rust_string(&handler.message_name),
            )
        }
    }

    pub(super) fn event_handler_rs(&self, handler: &MessageHandler) -> String {
        format!(
            r#"use distributed::microsvc::{{Context, HandlerError}};
use serde_json::{{json, Value}};

use crate::service::ServiceRepo;

pub const EVENT: &str = {message_name};

pub fn guard(_ctx: &Context<ServiceRepo>) -> bool {{
    true
}}

pub async fn handle(ctx: &Context<'_, ServiceRepo>) -> Result<Value, HandlerError> {{
    let input = ctx.input::<Value>()?;
    Ok(json!({{ "event": EVENT, "input": input }}))
}}
"#,
            message_name = rust_string(&handler.message_name),
        )
    }

    pub(super) fn read_models_mod_rs(&self) -> String {
        let views = self
            .read_models
            .iter()
            .map(|model| {
                format!(
                    r#"#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ReadModel)]
#[table({table_name})]
pub struct {view_struct} {{
    #[id("id")]
    pub id: String,
    pub name: String,
    pub status: String,
}}
"#,
                    table_name = rust_string(&model.table_name),
                    view_struct = model.view_ident,
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            r#"use distributed::ReadModel;
use serde::{{Deserialize, Serialize}};

{views}
"#
        )
    }

    pub(super) fn command_model(&self, handler: &MessageHandler) -> Option<&ModelScaffold> {
        if self.models.is_empty() {
            return None;
        }
        let message_model = message_owner(&handler.message_name);
        self.models
            .iter()
            .find(|model| model.name == message_model)
            .or_else(|| self.models.first())
    }
}
