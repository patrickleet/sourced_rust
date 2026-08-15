//! Framework runtime host.
//!
//! [`RuntimeHost::bind`] fails closed unless the explained capability set and
//! an explicit [`CommandDispatcher`] are present. Local SQL realization
//! (`realize_local`) starts the matching store/bus/workers and only advertises
//! capabilities that those adapters actually provide.

use super::capability::Capability;
use super::error::{ApplicationError, ApplicationResult};
use super::plan::{DeploymentPlan, ProcessPlan};
use super::render::NormalizedInventory;
use super::resolve::ResolvedDeployment;
use super::topology::TopologyIntent;
use crate::command_dispatch::SharedCommandDispatcher;
use std::collections::BTreeSet;

/// Provider-backed capabilities available to the host at bind time.
#[derive(Clone, Debug, Default)]
pub struct CapabilityProviders {
    pub available: BTreeSet<Capability>,
}

impl CapabilityProviders {
    pub fn with(mut self, capability: Capability) -> Self {
        self.available.insert(capability);
        self
    }

    pub fn contains(&self, capability: Capability) -> bool {
        self.available.contains(&capability)
    }
}

/// One bound process ready for supervision / serve (task 12 expansion point).
pub struct RuntimeHost {
    pub plan_name: String,
    pub process: ProcessPlan,
    pub dispatcher: Option<SharedCommandDispatcher>,
    pub providers: CapabilityProviders,
}

impl RuntimeHost {
    /// Bind one process from a validated plan against available providers.
    ///
    /// Missing required capabilities fail closed before serve.
    pub fn bind(
        plan: &DeploymentPlan,
        process_id: &str,
        providers: CapabilityProviders,
        dispatcher: Option<SharedCommandDispatcher>,
    ) -> ApplicationResult<Self> {
        plan.validate()?;
        let process = plan
            .processes
            .iter()
            .find(|process| process.id == process_id)
            .cloned()
            .ok_or_else(|| ApplicationError::Missing {
                kind: "process",
                identity: process_id.to_string(),
            })?;

        for requirement in &process.capabilities {
            if !providers.contains(requirement.capability) {
                return Err(ApplicationError::InvalidSpec(format!(
                    "process `{process_id}` requires capability `{}` but no provider is bound",
                    requirement.capability.as_str()
                )));
            }
        }

        let needs_dispatch = process.mounts.iter().any(|mount| {
            matches!(
                mount,
                super::mount::MountSelector::Command { .. }
                    | super::mount::MountSelector::Surface { .. }
            )
        });
        if needs_dispatch && dispatcher.is_none() {
            return Err(ApplicationError::InvalidSpec(format!(
                "process `{process_id}` requires a CommandDispatcher for command/surface mounts"
            )));
        }

        Ok(Self {
            plan_name: plan.name.clone(),
            process,
            dispatcher,
            providers,
        })
    }

    pub fn process_id(&self) -> &str {
        &self.process.id
    }

    pub fn dispatcher(&self) -> Option<&SharedCommandDispatcher> {
        self.dispatcher.as_ref()
    }

    /// Inventory contributed by this bound process (mounts, routes, subscriptions).
    pub fn process_inventory_parts(&self) -> (Vec<String>, Vec<String>, Vec<String>, Vec<String>) {
        let mut mounts = Vec::new();
        let mut routes = Vec::new();
        let mut subscriptions = Vec::new();
        for mount in &self.process.mounts {
            mounts.push(format!("{}:{}", self.process.id, mount.id()));
        }
        for intent in &self.process.topology {
            match intent {
                TopologyIntent::CommandRoute { command_id, .. } => {
                    routes.push(format!("{}:{}", self.process.id, command_id));
                }
                TopologyIntent::ProjectionSubscription { projection_id, .. } => {
                    subscriptions.push(format!("{}:{}", self.process.id, projection_id));
                }
                TopologyIntent::SurfaceEndpoint { surface_id, .. } => {
                    routes.push(format!("{}:surface:{}", self.process.id, surface_id));
                }
                TopologyIntent::ExtensionHook { extension_id, .. } => {
                    routes.push(format!("{}:extension:{}", self.process.id, extension_id));
                }
            }
        }
        let mut capabilities = self
            .process
            .capabilities
            .iter()
            .map(|requirement| requirement.capability.as_str().to_string())
            .collect::<Vec<_>>();
        mounts.sort();
        routes.sort();
        subscriptions.sort();
        capabilities.sort();
        capabilities.dedup();
        (mounts, routes, subscriptions, capabilities)
    }
}

/// Merge bound hosts for one resolved pair into the shared inventory shape.
///
/// Local realization is the set of successfully bound processes. The digest
/// and application/plan identity come from the same resolved pair the
/// renderers consume.
pub fn inventory_from_hosts(
    resolved: &ResolvedDeployment,
    hosts: &[&RuntimeHost],
) -> NormalizedInventory {
    let mut processes = hosts
        .iter()
        .map(|host| host.process.id.clone())
        .collect::<Vec<_>>();
    processes.sort();
    let mut mounts = Vec::new();
    let mut routes = Vec::new();
    let mut subscriptions = Vec::new();
    let mut capabilities = Vec::new();
    for host in hosts {
        let (host_mounts, host_routes, host_subscriptions, host_capabilities) =
            host.process_inventory_parts();
        mounts.extend(host_mounts);
        routes.extend(host_routes);
        subscriptions.extend(host_subscriptions);
        capabilities.extend(host_capabilities);
    }
    mounts.sort();
    routes.sort();
    subscriptions.sort();
    capabilities.sort();
    capabilities.dedup();
    NormalizedInventory {
        application: resolved.application.clone(),
        plan: resolved.plan.clone(),
        processes,
        mounts,
        routes,
        subscriptions,
        capabilities,
        digest: resolved.inventory_digest.clone(),
    }
}

/// Convenience: bind the sole process when a plan has exactly one entry.
pub fn bind_single_process(
    plan: &DeploymentPlan,
    providers: CapabilityProviders,
    dispatcher: Option<SharedCommandDispatcher>,
) -> ApplicationResult<RuntimeHost> {
    if plan.processes.len() != 1 {
        return Err(ApplicationError::InvalidSpec(
            "bind_single_process requires exactly one process in the plan".into(),
        ));
    }
    RuntimeHost::bind(plan, &plan.processes[0].id, providers, dispatcher)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::{
        compile_deployment_plan, Application, CommandDefinition, CommandSpec, CommandTypeSpec,
        ModelFieldSpec, ModelSpec, Module, ProcessIntent, ProcessPreset, ProjectionSpec,
    };
    use crate::graphql::CommandConsistency;
    use std::sync::Arc;

    fn manifest() -> crate::application::ApplicationManifest {
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
        Application::new("todo-app")
            .module(module)
            .build()
            .unwrap()
            .manifest()
            .clone()
    }

    #[test]
    fn host_fails_closed_without_required_capability() {
        let manifest = manifest();
        let plan = compile_deployment_plan(
            "local",
            &manifest,
            [ProcessIntent::with_preset("full", &manifest, ProcessPreset::Writer).unwrap()],
        )
        .unwrap();
        let err = match RuntimeHost::bind(&plan, "full", CapabilityProviders::default(), None) {
            Ok(_) => panic!("expected missing capability failure"),
            Err(error) => error,
        };
        assert!(err.to_string().contains("capability"));
    }

    #[test]
    fn host_binds_when_capabilities_and_dispatcher_present() {
        use crate::command_dispatch::{CommandDispatchError, CommandDispatcher};
        use crate::microsvc::{CommandRequest, CommandResponse};
        use async_trait::async_trait;
        
        struct Stub;
        #[async_trait]
        impl CommandDispatcher for Stub {
            async fn dispatch(
                &self,
                _request: &CommandRequest,
            ) -> Result<CommandResponse, CommandDispatchError> {
                Ok(CommandResponse {
                    status: 200,
                    body: serde_json::json!({}),
                })
            }
            fn kind(&self) -> &'static str {
                "stub"
            }
        }

        let manifest = manifest();
        let plan = compile_deployment_plan(
            "local",
            &manifest,
            [ProcessIntent::with_preset("full", &manifest, ProcessPreset::Writer).unwrap()],
        )
        .unwrap();
        let mut providers = CapabilityProviders::default();
        for requirement in &plan.processes[0].capabilities {
            providers = providers.with(requirement.capability);
        }
        let host = RuntimeHost::bind(
            &plan,
            "full",
            providers,
            Some(Arc::new(Stub) as SharedCommandDispatcher),
        )
        .unwrap();
        assert_eq!(host.process_id(), "full");
        assert!(host.dispatcher().is_some());
    }
}
