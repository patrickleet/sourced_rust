//! Placement-independent application composition.
//!
//! The application module is deliberately feature-light. It contains the
//! portable contract values used by contract-only packages as well as the
//! erased executable mounts used at a heterogeneous runtime boundary. SQL,
//! HTTP, GraphQL execution, brokers, and deployment clients are not required
//! to construct these values.

mod command;
mod error;
mod identity;
mod manifest;
mod module;
mod registration;

pub use command::{
    CommandMount, CommandSpec, CommandTypeField, CommandTypeSpec, EventSpec, TypeSpec,
};
pub use error::{ApplicationError, ApplicationResult};
pub use identity::{canonical_json, sha256_fingerprint, LogicalId};
pub use manifest::{
    ApplicationManifest, ManifestFingerprint, ManifestProvenance,
    APPLICATION_MANIFEST_SCHEMA_VERSION,
};
pub use module::{
    ModelFieldSpec, ModelSpec, Module, ModuleBuilder, ModuleManifest, ProjectionSpec,
    SurfaceCommandSpec, SurfaceSpec,
};
pub use registration::{Application, ApplicationBuilder, ContractCompiler};
