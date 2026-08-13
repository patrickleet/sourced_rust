//! Event-independent read-model mutation IR and interpreters.
//!
//! Mutations describe finite read-model changes without event selectors,
//! upcasters, owners, placement, or command-preview data. Declarative
//! [`crate::projection!`] bindings map domain events to mutation inputs; the
//! server and cache interpreters consume the same canonical IR.
//!
//! Bound mutations adapt to the projection operation vocabulary for physical
//! execution. The old event-owning projection proc-macro authoring path is gone.

#![deny(missing_docs)]
#![allow(clippy::type_complexity)]

mod bind;
mod cache;
mod canonical;
mod capabilities;
mod descriptor;
mod error;
mod expression;
mod handler;
mod preview;
mod program;
#[cfg(test)]
mod relationship_fixture;
mod server;

#[cfg(test)]
mod tests;

pub use bind::{
    body_field_binding, envelope_binding, identity_body_path_binder, required_input_paths,
    MutationEventBinding, MutationInputBinding,
};
pub use cache::{
    is_cache_writable, lower_mutation_cache, lower_projection_ops_cache, presence_label,
    MutationCacheEffect, MutationCacheProgram, MutationCacheVisibility,
};
pub use capabilities::{
    MutationFieldCapability, MutationKeyCapability, MutationModelIdentity,
    MutationRelationshipCapability, ReadModelMutationCapabilities,
};
pub use descriptor::{
    assert_mutation_backed_program, bind_delete_to_envelope_id, bind_event_apply_mutation,
    bind_event_to_mutation, bind_state_body_to_mutation, bind_state_events_to_mutation,
    body_bindings_for_model, compile_projection, delete_by_pk_program_for_model,
    descriptor_from_factories, inventory_single_model, lower_single_model,
    projection_value_type_for_column, resolve_mutation_program, state_upsert_program_for_model,
    ProjectionHandler, ProjectionInputSource,
};
pub use error::MutationProgramError;
pub use expression::{
    MutationAssignment, MutationExpression, MutationExpressionObjectField, ResolvedMutationValue,
    MAX_MUTATION_EXPRESSION_DEPTH, MAX_MUTATION_PATH_SEGMENTS,
};
pub use handler::{
    bindings_from_expressions, portable_binding, CustomMutationHandler, MutationHandlerCatalog,
    MutationHandlerPlacement, MutationHandlerRegistration, MutationHandlerUniquenessKey,
};
pub use preview::{
    causal_scopes, compose_event_preview, reconcile_with_actual, rewrite_binding_ops,
    zero_binding_preview, ComposedPreviewLayer, MutationCausalScope, PreviewOwnerContribution,
};
pub use program::{
    Mutation, MutationConflictTarget, MutationField, MutationKeyField, MutationKind,
    MutationOperation, MutationProgram, MutationProgramId, MutationProgramLimits,
    MutationReturning, MAX_MUTATION_OPERATIONS, MUTATION_OPERATION_SEMANTICS_VERSION,
    MUTATION_PROGRAM_IR_VERSION,
};
pub use server::{rewrite_program_with_binder, simple_body_bindings, MutationServerInterpreter};
