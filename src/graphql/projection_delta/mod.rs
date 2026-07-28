//! Role-safe, deterministic client projection deltas.
//!
//! This module lowers logical projection provenance through an explicit
//! authorization boundary. It never derives client semantics from physical
//! table write plans.

pub(crate) mod authorization;
mod canonical;
pub(crate) mod lower;
mod types;

#[cfg(test)]
mod tests;

pub use types::{
    AuthorizationTransition, DeltaField, DeltaKeyField, DeltaValue, ProjectionDelta,
    ProjectionDeltaError, ProjectionDeltaIdentity, ProjectionDeltaMutation,
    ProjectionDeltaOccurrence, ProjectionDeltaOperation, ProjectionDeltaPartition,
    ProjectionDeltaProjectionIdentity, ProjectionDeltaRecovery, ProjectionDeltaRecoveryTarget,
    ProjectionDeltaScope, ProjectionDeltaSurfaceIdentity, ProjectionDeltaVisibility,
    PROJECTION_DELTA_WIRE_VERSION,
};
