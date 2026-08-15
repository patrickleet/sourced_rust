//! Placement-independent application composition.
//!
//! The application module is deliberately feature-light. It contains the
//! portable contract values used by contract-only packages as well as the
//! erased executable mounts used at a heterogeneous runtime boundary. SQL,
//! HTTP, GraphQL execution, brokers, and deployment clients are not required
//! to construct these values.

mod capability;
mod command;
mod error;
mod identity;
mod manifest;
mod module;
mod mount;
mod plan;
mod registration;
mod render;
mod resolve;
mod runtime_host;
#[cfg(all(
    feature = "graphql",
    feature = "http",
    any(feature = "sqlite", feature = "postgres")
))]
mod local_sql;
mod topology;

pub use capability::{
    Capability, CapabilityReason, CapabilityRequirement, SchemaLifecycleRequirement,
};
pub use command::{
    CommandDefinition, CommandMount, CommandMountFuture, CommandMountHandler, CommandMountRegistrar,
    CommandSpec, CommandTypeField, CommandTypeSpec, EventSpec, TypeSpec,
};
pub(crate) use command::{
    CommandMountExecution, CommandMountExecutionError, CommandMountExecutionFuture,
    CommandMountExecutionResult, CommandMountInvocation,
};
pub use error::{ApplicationError, ApplicationResult};
pub use identity::{canonical_json, sha256_fingerprint, LogicalId};
pub use manifest::{
    ApplicationExtension, ApplicationManifest, ManifestFingerprint, ManifestProvenance,
    APPLICATION_MANIFEST_SCHEMA_VERSION, MAX_APPLICATION_MANIFEST_BYTES,
    MAX_MANIFEST_COLLECTION_ITEMS, MAX_MANIFEST_JSON_BYTES, MAX_MANIFEST_JSON_DEPTH,
    MAX_MANIFEST_STRING_BYTES,
};
pub use module::{
    ModelFieldSpec, ModelRelationshipSpec, ModelSpec, Module, ModuleBuilder, ModuleManifest,
    ProjectionSpec, SurfaceAggregateSpec, SurfaceArgumentSpec, SurfaceCommandSpec, SurfaceRootSpec,
    SurfaceSpec,
};
pub use mount::{MountSelector, ProcessPreset};
pub use plan::{
    compile_deployment_plan, DeploymentPlan, PlanFingerprint, ProcessIntent, ProcessPlan,
    DEPLOYMENT_PLAN_SCHEMA_VERSION, MAX_DEPLOYMENT_PLAN_BYTES,
};
pub use registration::{Application, ApplicationBuilder, ContractCompiler};
pub use render::{
    inventory_from_rendered, normalize_resolved, render_resolved, NormalizedInventory, RenderTarget,
    RenderedFile,
};
pub use resolve::{
    resolve_deployment, ResolvedDeployment, ResolvedProcess, RESOLVED_DEPLOYMENT_SCHEMA_VERSION,
};
pub use runtime_host::{
    bind_single_process, inventory_from_hosts, CapabilityProviders, RuntimeHost,
};
#[cfg(all(
    feature = "graphql",
    feature = "http",
    any(feature = "sqlite", feature = "postgres")
))]
pub use local_sql::{
    LocalSqlApplication, LocalSqlHandles, LocalSqlOptions, RealizedRuntimeHost,
};
pub use topology::TopologyIntent;

/// Compile-time duplicate check emitted by the module macro for generated
/// command definition IDs. It compares the complete IDs, not a truncated or
/// hashed surrogate, so a hash collision cannot reject a valid module or hide
/// a duplicate declaration.
pub const fn assert_unique_command_ids(ids: &[&str]) {
    let mut index = 0;
    while index < ids.len() {
        let mut other = index + 1;
        while other < ids.len() {
            if same_const_str(ids[index], ids[other]) {
                panic!("duplicate command identity in module declaration");
            }
            other += 1;
        }
        index += 1;
    }
}

const fn same_const_str(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.len() != right.len() {
        return false;
    }
    let mut index = 0;
    while index < left.len() {
        if left[index] != right[index] {
            return false;
        }
        index += 1;
    }
    true
}
