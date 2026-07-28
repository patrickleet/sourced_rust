//! Role-safe, deterministic client projection deltas.
//!
//! This module lowers logical projection provenance through an explicit
//! authorization boundary. It never derives client semantics from physical
//! table write plans.

mod authorization;
mod canonical;
mod lower;
mod types;

#[cfg(test)]
mod tests;

pub use authorization::{
    AuthorizedField, AuthorizedModel, AuthorizedRecordKey, AuthorizedRelationship,
    ProjectionAuthorization,
};
pub use lower::{
    lower_projection_delta, LogicalFieldValue, LogicalProjectionBatch,
    LogicalProjectionInvalidation, LogicalProjectionMutation, LogicalProjectionOccurrence,
    LogicalRelationshipEffect, ProjectionDeltaContext, ProjectionDeltaSource,
};
pub use types::{
    AuthorizationTransition, DeltaField, DeltaKeyField, DeltaValue, ProjectionDelta,
    ProjectionDeltaError, ProjectionDeltaIdentity, ProjectionDeltaMutation,
    ProjectionDeltaOccurrence, ProjectionDeltaOperation, ProjectionDeltaPartition,
    ProjectionDeltaProjectionIdentity, ProjectionDeltaRecovery, ProjectionDeltaRecoveryTarget,
    ProjectionDeltaScope, ProjectionDeltaSurfaceIdentity, ProjectionDeltaVisibility,
    ProjectionMutationSource, PROJECTION_DELTA_WIRE_VERSION,
};
