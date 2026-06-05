//! Internal generation: build a normalized `Scaffold` from a spec and render the
//! project files. Pure — every method returns `String`s; nothing touches the
//! filesystem. Ported from the original `hops-cli service scaffold` command.

use std::collections::BTreeSet;

use crate::{
    BusTarget, GeneratedFile, GeneratedProject, GithubRepo, GithubScaffoldSpec,
    GitopsPromoteTarget, PostCreateAction, ScaffoldError, ServiceScaffoldSpec, ServiceTransport,
    StoreTarget,
};

/// Generate a Distributed service project from a spec. The public entry point.
pub fn generate_service_scaffold(
    spec: ServiceScaffoldSpec,
) -> Result<GeneratedProject, ScaffoldError> {
    Ok(Scaffold::from_spec(spec)?.generate())
}

/// Parse an `owner/repo` string (used by [`GithubRepo::parse`]).
pub(crate) fn parse_github_repo(raw: &str) -> Result<GithubRepo, ScaffoldError> {
    let trimmed = raw.trim();
    let Some((owner, repo)) = trimmed.split_once('/') else {
        return Err(ScaffoldError::new("repository must be in OWNER/REPO form"));
    };
    if owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return Err(ScaffoldError::new("repository must be in OWNER/REPO form"));
    }
    let valid = [owner, repo].into_iter().all(|part| {
        part.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    });
    if !valid {
        return Err(ScaffoldError::new(
            "repository contains unsupported GitHub characters",
        ));
    }
    Ok(GithubRepo {
        owner: owner.to_string(),
        repo: repo.to_string(),
    })
}

struct Scaffold {
    names: ScaffoldNames,
    distributed_dependency_path: String,
    transport: ServiceTransport,
    store: StoreTarget,
    #[allow(dead_code)] // used by GitOps/Knative generation (follow-up slice)
    bus: Option<BusTarget>,
    include_read_models: bool,
    gitops: bool,
    gitops_promote: Option<GitopsPromoteTarget>,
    github: Option<GithubScaffoldSpec>,
    models: Vec<ModelScaffold>,
    read_models: Vec<ModelScaffold>,
    commands: Vec<MessageHandler>,
    events: Vec<MessageHandler>,
}

impl Scaffold {
    fn from_spec(spec: ServiceScaffoldSpec) -> Result<Self, ScaffoldError> {
        let names = ScaffoldNames::new(&spec.name)?;
        let models = model_scaffolds(&spec.models)?;
        let read_models = if spec.read_models {
            if models.is_empty() {
                vec![ModelScaffold::new(&names.package_name)?]
            } else {
                models.clone()
            }
        } else {
            Vec::new()
        };
        let mut module_idents = BTreeSet::new();
        let commands = message_handlers_with_modules(
            if spec.commands.is_empty() {
                vec![default_command_name(&names, &models)]
            } else {
                spec.commands.clone()
            },
            "command",
            &mut module_idents,
        )?;
        let events =
            message_handlers_with_modules(spec.events.clone(), "event", &mut module_idents)?;
        Ok(Self {
            names,
            distributed_dependency_path: spec.distributed_dependency_path,
            transport: spec.transport,
            store: spec.store,
            bus: spec.bus,
            include_read_models: spec.read_models,
            gitops: spec.gitops,
            gitops_promote: spec.gitops_promote,
            github: spec.github,
            models,
            read_models,
            commands,
            events,
        })
    }

    fn generate(self) -> GeneratedProject {
        let mut files = Vec::new();
        let mut warnings = Vec::new();
        let mut post_create_actions = Vec::new();

        files.push(file("Cargo.toml", self.cargo_toml()));
        files.push(file("src/lib.rs", self.lib_rs()));
        files.push(file("src/main.rs", self.main_rs()));
        files.push(file("src/manifest.rs", self.manifest_rs()));
        files.push(file("src/service.rs", self.service_rs()));
        if !self.models.is_empty() {
            files.push(file("src/models/mod.rs", self.models_mod_rs()));
            for model in &self.models {
                files.push(file(
                    &format!("src/models/{}.rs", model.module_ident),
                    self.model_rs(model),
                ));
            }
        }
        files.push(file("src/handlers/mod.rs", self.handlers_mod_rs()));
        for command in &self.commands {
            files.push(file(
                &format!("src/handlers/{}.rs", command.module_ident),
                self.command_handler_rs(command),
            ));
        }
        for event in &self.events {
            files.push(file(
                &format!("src/handlers/{}.rs", event.module_ident),
                self.event_handler_rs(event),
            ));
        }
        if self.include_read_models {
            files.push(file("src/read_models/mod.rs", self.read_models_mod_rs()));
        }

        // GitOps deploy/promote charts and GitHub workflow files are a follow-up
        // slice; the spec fields are accepted so the API is stable. Until then,
        // surface a warning so the caller knows those artifacts were not emitted.
        if self.gitops || self.gitops_promote.is_some() || self.github.is_some() {
            warnings.push(
                "GitOps and GitHub workflow generation are not yet ported into \
                 distributed_tooling; those artifacts were not generated"
                    .to_string(),
            );
        }
        if let Some(github) = &self.github {
            post_create_actions.push(PostCreateAction::EnsureGithubRepository {
                repo: github.repository.clone(),
            });
        }

        GeneratedProject {
            files,
            warnings,
            post_create_actions,
        }
    }

    fn cargo_toml(&self) -> String {
        let distributed_path = toml_string(&self.distributed_dependency_path);
        let features = self
            .distributed_features()
            .into_iter()
            .map(toml_string)
            .collect::<Vec<_>>()
            .join(", ");
        let axum = if self.transport == ServiceTransport::Knative {
            "axum = \"0.7\"\n"
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
        features
    }

    fn lib_rs(&self) -> String {
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

    fn main_rs(&self) -> String {
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

    fn manifest_rs(&self) -> String {
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

    fn service_rs(&self) -> String {
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

use distributed::{{microsvc::Service, HashMapRepository, ServiceManifest}};

use crate::handlers;

pub type ServiceRepo = HashMapRepository;

pub fn in_memory() -> Arc<Service<ServiceRepo>> {{
    build(HashMapRepository::new())
}}

pub fn build(repo: ServiceRepo) -> Arc<Service<ServiceRepo>> {{
    Arc::new(distributed::register_handlers!(
        Service::new().with_repo(repo),
{registrations}    ))
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

    fn models_mod_rs(&self) -> String {
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

    fn model_rs(&self, model: &ModelScaffold) -> String {
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

    fn handlers_mod_rs(&self) -> String {
        self.commands
            .iter()
            .chain(self.events.iter())
            .map(|handler| format!("pub mod {};\n", handler.module_ident))
            .collect()
    }

    fn command_handler_rs(&self, handler: &MessageHandler) -> String {
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

    fn event_handler_rs(&self, handler: &MessageHandler) -> String {
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

    fn read_models_mod_rs(&self) -> String {
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

    fn command_model(&self, handler: &MessageHandler) -> Option<&ModelScaffold> {
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

fn file(path: &str, contents: String) -> GeneratedFile {
    GeneratedFile {
        path: path.to_string(),
        contents,
        mode: None,
    }
}

// ---------------------------------------------------------------------------
// Normalization helpers (the pure rules this crate owns)
// ---------------------------------------------------------------------------

struct ScaffoldNames {
    package_name: String,
    crate_ident: String,
    command_name: String,
}

impl ScaffoldNames {
    fn new(input: &str) -> Result<Self, ScaffoldError> {
        let package_name = to_kebab_case(input);
        if package_name.is_empty() {
            return Err(ScaffoldError::new(
                "service name must contain at least one ASCII letter or digit",
            ));
        }
        let crate_ident = package_name.replace('-', "_");
        let command_name = format!("{crate_ident}.create");
        Ok(Self {
            package_name,
            crate_ident,
            command_name,
        })
    }
}

#[derive(Clone, Debug)]
struct ModelScaffold {
    name: String,
    message_prefix: String,
    module_ident: String,
    type_ident: String,
    view_ident: String,
    table_name: String,
}

impl ModelScaffold {
    fn new(raw_name: &str) -> Result<Self, ScaffoldError> {
        let name = to_kebab_case(raw_name);
        if name.is_empty() {
            return Err(ScaffoldError::new(
                "model name must contain at least one ASCII letter or digit",
            ));
        }
        let ident = name.replace('-', "_");
        let type_ident = to_pascal_case(&name);
        let view_ident = format!("{type_ident}View");
        Ok(Self {
            name: name.clone(),
            message_prefix: name.clone(),
            module_ident: ident.clone(),
            type_ident,
            view_ident,
            table_name: format!("{ident}_views"),
        })
    }
}

fn model_scaffolds(raw_models: &[String]) -> Result<Vec<ModelScaffold>, ScaffoldError> {
    let mut seen = BTreeSet::new();
    let mut models = Vec::new();
    for raw_model in raw_models {
        let model = ModelScaffold::new(raw_model)?;
        if !seen.insert(model.name.clone()) {
            return Err(ScaffoldError::new(format!(
                "duplicate model `{}`",
                model.name
            )));
        }
        models.push(model);
    }
    Ok(models)
}

fn default_command_name(names: &ScaffoldNames, models: &[ModelScaffold]) -> String {
    models
        .first()
        .map(|model| format!("{}.create", model.name))
        .unwrap_or_else(|| names.command_name.clone())
}

#[derive(Clone, Debug)]
struct MessageHandler {
    message_name: String,
    module_ident: String,
}

fn message_handlers_with_modules(
    names: Vec<String>,
    fallback_prefix: &str,
    seen_modules: &mut BTreeSet<String>,
) -> Result<Vec<MessageHandler>, ScaffoldError> {
    let mut seen_names = BTreeSet::new();
    let mut handlers = Vec::new();
    for raw_name in names {
        let message_name = raw_name.trim();
        validate_message_name(message_name, fallback_prefix)?;
        if !seen_names.insert(message_name.to_string()) {
            return Err(ScaffoldError::new(format!(
                "duplicate {fallback_prefix} `{message_name}`"
            )));
        }
        let base_module = to_rust_ident(message_name, fallback_prefix);
        let mut module_ident = base_module.clone();
        let mut suffix = 2;
        while !seen_modules.insert(module_ident.clone()) {
            module_ident = format!("{base_module}_{suffix}");
            suffix += 1;
        }
        handlers.push(MessageHandler {
            message_name: message_name.to_string(),
            module_ident,
        });
    }
    Ok(handlers)
}

fn validate_message_name(name: &str, kind: &str) -> Result<(), ScaffoldError> {
    if name.is_empty() {
        return Err(ScaffoldError::new(format!("{kind} name cannot be empty")));
    }
    if name.chars().any(char::is_control) {
        return Err(ScaffoldError::new(format!(
            "{kind} `{name}` contains a control character"
        )));
    }
    Ok(())
}

fn message_owner(message_name: &str) -> String {
    message_name
        .split('.')
        .find(|part| !part.is_empty())
        .map(k8s_name)
        .unwrap_or_else(|| "message".to_string())
}

fn k8s_name(value: &str) -> String {
    let name = to_kebab_case(value);
    if name.is_empty() {
        "generated".to_string()
    } else {
        name
    }
}

fn to_rust_ident(value: &str, fallback_prefix: &str) -> String {
    let mut ident = String::new();
    let mut last_was_separator = false;
    for char in value.chars() {
        if char.is_ascii_alphanumeric() {
            ident.push(char.to_ascii_lowercase());
            last_was_separator = false;
        } else if !last_was_separator {
            ident.push('_');
            last_was_separator = true;
        }
    }
    while ident.ends_with('_') {
        ident.pop();
    }
    while ident.starts_with('_') {
        ident.remove(0);
    }
    if ident.is_empty() {
        ident = fallback_prefix.to_string();
    }
    if ident
        .chars()
        .next()
        .is_some_and(|char| char.is_ascii_digit())
        || is_rust_keyword(&ident)
    {
        ident = format!("{fallback_prefix}_{ident}");
    }
    ident
}

fn is_rust_keyword(value: &str) -> bool {
    matches!(
        value,
        "as" | "break"
            | "const"
            | "continue"
            | "crate"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "Self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "async"
            | "await"
            | "dyn"
    )
}

fn to_kebab_case(input: &str) -> String {
    let mut out = String::new();
    let mut last_was_separator = true;
    for char in input.chars() {
        if char.is_ascii_alphanumeric() {
            out.push(char.to_ascii_lowercase());
            last_was_separator = false;
        } else if !last_was_separator {
            out.push('-');
            last_was_separator = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

fn to_pascal_case(input: &str) -> String {
    input
        .split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            let Some(first) = chars.next() else {
                return String::new();
            };
            let mut out = String::new();
            out.push(first.to_ascii_uppercase());
            out.extend(chars);
            out
        })
        .collect()
}

fn toml_string(value: impl AsRef<str>) -> String {
    serde_json::to_string(value.as_ref()).expect("string serialization should succeed")
}

fn rust_string(value: &str) -> String {
    toml_string(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &str) -> ServiceScaffoldSpec {
        ServiceScaffoldSpec {
            name: name.to_string(),
            transport: ServiceTransport::Http,
            store: StoreTarget::Postgres,
            bus: None,
            models: Vec::new(),
            read_models: false,
            commands: Vec::new(),
            events: Vec::new(),
            distributed_dependency_path: "../distributed".to_string(),
            gitops: false,
            gitops_promote: None,
            github: None,
        }
    }

    fn paths(project: &GeneratedProject) -> Vec<&str> {
        project.files.iter().map(|f| f.path.as_str()).collect()
    }

    fn contents<'a>(project: &'a GeneratedProject, path: &str) -> &'a str {
        project
            .files
            .iter()
            .find(|f| f.path == path)
            .map(|f| f.contents.as_str())
            .unwrap_or_else(|| panic!("missing file {path}"))
    }

    #[test]
    fn generates_the_core_service_crate() {
        let project = generate_service_scaffold(spec("orders")).unwrap();
        let paths = paths(&project);
        for expected in [
            "Cargo.toml",
            "src/lib.rs",
            "src/main.rs",
            "src/manifest.rs",
            "src/service.rs",
            "src/handlers/mod.rs",
        ] {
            assert!(paths.contains(&expected), "missing {expected} in {paths:?}");
        }
        // A default command handler is derived when none is given.
        assert!(paths
            .iter()
            .any(|p| p.starts_with("src/handlers/") && *p != "src/handlers/mod.rs"));
    }

    #[test]
    fn service_uses_the_new_builder_api() {
        let project = generate_service_scaffold(spec("orders")).unwrap();
        let service = contents(&project, "src/service.rs");
        assert!(
            service.contains("Service::new().with_repo(repo)"),
            "service.rs should use the new builder API:\n{service}"
        );
        assert!(!service.contains("Service::with_repo("));
    }

    #[test]
    fn cargo_features_track_transport_and_store() {
        let mut s = spec("orders");
        s.store = StoreTarget::Sqlite;
        let project = generate_service_scaffold(s).unwrap();
        let cargo = contents(&project, "Cargo.toml");
        assert!(cargo.contains("\"http\""));
        assert!(cargo.contains("\"sqlite\""));
    }

    #[test]
    fn read_models_and_models_emit_modules() {
        let mut s = spec("orders");
        s.models = vec!["order".to_string()];
        s.read_models = true;
        let project = generate_service_scaffold(s).unwrap();
        let paths = paths(&project);
        assert!(paths.contains(&"src/models/mod.rs"));
        assert!(paths.contains(&"src/models/order.rs"));
        assert!(paths.contains(&"src/read_models/mod.rs"));
    }

    #[test]
    fn github_yields_a_post_create_action_and_warning() {
        let mut s = spec("orders");
        s.github = Some(GithubScaffoldSpec {
            repository: GithubRepo::parse("hops-ops/orders").unwrap(),
            preview_environment_repository: None,
            promote_environment_repository: None,
        });
        let project = generate_service_scaffold(s).unwrap();
        assert_eq!(
            project.post_create_actions,
            vec![PostCreateAction::EnsureGithubRepository {
                repo: GithubRepo {
                    owner: "hops-ops".to_string(),
                    repo: "orders".to_string(),
                },
            }]
        );
        assert!(!project.warnings.is_empty());
    }

    #[test]
    fn invalid_name_is_an_error() {
        assert!(generate_service_scaffold(spec("   ")).is_err());
    }

    #[test]
    fn github_repo_parse_rejects_bad_input() {
        assert!(GithubRepo::parse("no-slash").is_err());
        assert!(GithubRepo::parse("a/b/c").is_err());
        assert_eq!(GithubRepo::parse("o/r").unwrap().slug(), "o/r");
    }
}
