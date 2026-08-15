//! Renderer-neutral resolved deployment from one manifest + plan pair.
//!
//! Local hosts, raw Kubernetes/Knative renderers, and the offline Hops XR
//! consume this graph. They must not invent a second semantic inventory.

use serde::{Deserialize, Serialize};

use super::capability::CapabilityRequirement;
use super::error::{ApplicationError, ApplicationResult};
use super::identity::{canonical_json, sha256_fingerprint};
use super::manifest::ApplicationManifest;
use super::mount::MountSelector;
use super::plan::DeploymentPlan;
use super::topology::TopologyIntent;

/// Schema version for the resolved deployment artifact.
pub const RESOLVED_DEPLOYMENT_SCHEMA_VERSION: u32 = 1;

/// One resolved process ready for host bind or renderer emission.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedProcess {
    pub id: String,
    pub mounts: Vec<MountSelector>,
    pub remote_commands: bool,
    pub capabilities: Vec<CapabilityRequirement>,
    pub topology: Vec<TopologyIntent>,
}

/// Renderer-neutral graph of processes, routes, subscriptions, capabilities,
/// and compatibility digests for one validated plan.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedDeployment {
    pub schema_version: u32,
    pub application: String,
    pub plan: String,
    pub application_manifest_logical: String,
    pub application_manifest_canonical: String,
    pub plan_logical: String,
    pub plan_canonical: String,
    pub processes: Vec<ResolvedProcess>,
    pub capabilities: Vec<CapabilityRequirement>,
    pub inventory_digest: String,
}

impl ResolvedDeployment {
    /// Canonical bytes used for inventory comparison across renderers.
    pub fn inventory_bytes(&self) -> ApplicationResult<Vec<u8>> {
        #[derive(Serialize)]
        struct Inventory<'a> {
            application: &'a str,
            plan: &'a str,
            application_manifest_canonical: &'a str,
            plan_canonical: &'a str,
            processes: &'a [ResolvedProcess],
            capabilities: &'a [CapabilityRequirement],
        }
        let value = serde_json::to_value(&Inventory {
            application: &self.application,
            plan: &self.plan,
            application_manifest_canonical: &self.application_manifest_canonical,
            plan_canonical: &self.plan_canonical,
            processes: &self.processes,
            capabilities: &self.capabilities,
        })
        .map_err(|error| ApplicationError::Canonical(error.to_string()))?;
        serde_json::to_vec(&canonical_json(&value))
            .map_err(|error| ApplicationError::Canonical(error.to_string()))
    }
}

/// Resolve one unchanged manifest + plan into the shared semantic graph.
///
/// Fails closed when the plan's predecessor identity does not match the
/// supplied manifest.
pub fn resolve_deployment(
    manifest: &ApplicationManifest,
    plan: &DeploymentPlan,
) -> ApplicationResult<ResolvedDeployment> {
    manifest.validate()?;
    plan.validate()?;
    let expected_logical = manifest.logical_fingerprint()?;
    let expected_canonical = manifest.fingerprint()?;
    if plan.application_manifest_logical != expected_logical
        || plan.application_manifest_canonical != expected_canonical
    {
        return Err(ApplicationError::InvalidSpec(format!(
            "deployment plan `{}` predecessor does not match application `{}`",
            plan.name, manifest.name
        )));
    }
    if plan.application != manifest.name {
        return Err(ApplicationError::InvalidSpec(format!(
            "deployment plan application `{}` does not match manifest `{}`",
            plan.application, manifest.name
        )));
    }
    let processes = plan
        .processes
        .iter()
        .map(|process| ResolvedProcess {
            id: process.id.clone(),
            mounts: process.mounts.clone(),
            remote_commands: process.remote_commands,
            capabilities: process.capabilities.clone(),
            topology: process.topology.clone(),
        })
        .collect::<Vec<_>>();
    let mut resolved = ResolvedDeployment {
        schema_version: RESOLVED_DEPLOYMENT_SCHEMA_VERSION,
        application: manifest.name.clone(),
        plan: plan.name.clone(),
        application_manifest_logical: expected_logical,
        application_manifest_canonical: expected_canonical,
        plan_logical: plan.fingerprints.logical.clone(),
        plan_canonical: plan.fingerprints.canonical.clone(),
        processes,
        capabilities: plan.capabilities.clone(),
        inventory_digest: String::new(),
    };
    let inventory = resolved.inventory_bytes()?;
    resolved.inventory_digest = sha256_fingerprint(&inventory);
    Ok(resolved)
}
