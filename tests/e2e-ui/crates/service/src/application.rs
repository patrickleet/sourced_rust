//! e2e-ui application composition root.
//!
//! This is the review-visible product declaration: surface identities, module
//! inventory, and the validated local deployment plan. Infrastructure
//! (dialect, outbox, OIDC serve) stays in `host`; handlers stay in modules.

use distributed::application::{
    compile_deployment_plan, Application, ApplicationManifest, DeploymentPlan, ModelFieldSpec,
    ModelSpec, Module, ProcessIntent, ProcessPreset, ProjectionSpec,
};

use crate::modules::{blob, chat, compose, contracts, todo};

/// Stable normal-application surface shared by user and admin sessions.
pub const DISTRIBUTED_CLIENT_SURFACE: &str = "e2e-ui";
/// Stable elevated surface for routes that intentionally include admin-only fields.
pub const DISTRIBUTED_ADMIN_CLIENT_SURFACE: &str = "e2e-ui-admin";
/// Unauthenticated public surface (lobby message peek).
pub const DISTRIBUTED_PUBLIC_CLIENT_SURFACE: &str = "e2e-ui-public";

/// Logical application name used for manifest / plan identity.
pub const E2E_UI_APPLICATION: &str = "e2e-ui";

/// Explicit module identities owned by the e2e application.
pub const E2E_UI_MODULE_IDS: &[&str] = compose::MODULE_IDS;

/// Compile-time proof that module inventory matches bounded-context crates.
pub const MODULE_DECLARATIONS: &[(&str, &str)] = &[
    (todo::MODULE_ID, "todo commands + projector"),
    (chat::MODULE_ID, "chat commands + Zitadel extension + projectors"),
    (blob::MODULE_ID, "blob Atomic commands"),
    ("identity", "AuthUsers projection via chat module ingestors"),
];

fn model(id: &str, table: &str, pk: &str) -> ModelSpec {
    ModelSpec::try_new(
        id,
        table,
        [ModelFieldSpec {
            name: pk.into(),
            scalar: "String".into(),
            nullable: false,
        }],
        [pk],
    )
    .expect("e2e-ui model")
}

/// Placement inventory for the local host plan (contract modules plus
/// projector/model mounts required for Atomic collocation).
fn host_modules() -> Vec<Module> {
    let mut modules = contracts::application_modules();
    modules.push(
        Module::new("placement")
            .models([
                model("Todos", "todos", "todo_id"),
                model("ChatMessages", "chat_messages", "message_id"),
                model("BlobGames", "blob_games", "game_id"),
            ])
            .projections([
                ProjectionSpec::try_new("project_todos", ["todo.created"], ["Todos"])
                    .expect("todo projection"),
                ProjectionSpec::try_new(
                    "project_chat_messages",
                    ["chat.posted"],
                    ["ChatMessages"],
                )
                .expect("chat projection"),
                ProjectionSpec::try_new("project_blob", ["blob.started"], ["BlobGames"])
                    .expect("blob projection")
                    .with_direct(true)
                    .expect("blob direct projection"),
            ])
            .build()
            .expect("e2e-ui placement module"),
    );
    modules
}

/// Compile the portable e2e-ui application from contract + placement modules.
pub fn e2e_application() -> Application {
    Application::new(E2E_UI_APPLICATION)
        .modules(host_modules())
        .build()
        .expect("e2e-ui application contracts should compile")
}

/// Validated one-process local plan used by the framework host.
pub fn e2e_local_plan() -> (ApplicationManifest, DeploymentPlan) {
    let application = e2e_application();
    let manifest = application.manifest().clone();
    let plan = compile_deployment_plan(
        "local",
        &manifest,
        [ProcessIntent::with_preset("full", &manifest, ProcessPreset::All)
            .expect("e2e-ui full process intent")],
    )
    .expect("e2e-ui local deployment plan");
    (manifest, plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use distributed::application::{bind_single_process, resolve_deployment, CapabilityProviders};
    use distributed::command_dispatch::LocalCommandDispatcher;
    use distributed::microsvc::Service;
    use std::sync::Arc;

    #[test]
    fn local_plan_binds_runtime_host() {
        let (manifest, plan) = e2e_local_plan();
        let resolved = resolve_deployment(&manifest, &plan).unwrap();
        assert_eq!(resolved.application, E2E_UI_APPLICATION);
        assert_eq!(plan.processes.len(), 1);
        assert_eq!(plan.processes[0].id, "full");
        let mut providers = CapabilityProviders::default();
        for requirement in &plan.capabilities {
            providers = providers.with(requirement.capability);
        }
        let dispatcher = Arc::new(LocalCommandDispatcher::new(Arc::new(Service::new())));
        let host = bind_single_process(&plan, providers, Some(dispatcher)).unwrap();
        assert_eq!(host.process_id(), "full");
        assert!(host.dispatcher().is_some());
    }
}
