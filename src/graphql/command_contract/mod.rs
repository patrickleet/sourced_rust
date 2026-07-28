//! Typed command consistency, prepared completions, and portable client effects.
//!
//! This module deliberately separates declaration from durable completion. A
//! handler may prepare a typed payload, but it cannot choose which projection
//! confirmations count. That finite plan belongs to the command declaration,
//! and only the framework-owned command-ledger committer may turn a preparation into
//! an [`Succeeded`], [`Causal`], or [`Projected`] value.

#![cfg_attr(not(feature = "graphql"), allow(dead_code))]

mod direct_projection;
mod effect_wire;
mod effects;
mod outcomes;
mod projection_obligations;
mod projection_proof;
mod typed_command;

#[cfg(test)]
mod tests;

pub use direct_projection::CompiledDirectProjectionTarget;
pub(crate) use direct_projection::{
    compiled_direct_projection_target, CommandDirectProjectionTarget, CommandProjectedModel,
    ResolvedDirectProjectionTarget,
};
pub(crate) use effect_wire::compiled_projection_confirmation;
pub use effect_wire::{
    __command_confirmations, __command_effects, __command_input_defaults, __effect_assignment,
    __effect_constant, __effect_delete, __effect_input, __effect_invalidate_model,
    __effect_invalidate_relationship, __effect_key, __effect_key_assignment, __effect_key_field,
    __effect_link, __effect_null, __effect_patch, __effect_relationship, __effect_trusted,
    __effect_unlink, __effect_upsert, __input_default_ulid, __input_default_uuid_v7,
    CombineEffectNullability, CompiledCommandEffects, CompiledConfirmationPlan,
    CompiledEffectFieldValue, CompiledEffectKeyField, CompiledEffectOperation,
    CompiledInputDefault, CompiledInputDefaults, CompiledProjectionConfirmation,
    EffectAssignmentExpression, EffectInputDescendableKind, EffectInputFieldMarker,
    EffectInputObjectKind, EffectInputPath, EffectInputPathKind, EffectInputTerminalKind,
    EffectModelFieldMarker, EffectNullable, EffectPathNullability, EffectRelationshipMarker,
    EffectRequired, EffectWireBigInt, EffectWireBoolean, EffectWireBytea, EffectWireChecked,
    EffectWireCompatible, EffectWireFloat, EffectWireJson, EffectWireList, EffectWireLiteral,
    EffectWireObject, EffectWireString, EffectWireTimestamp, EffectWireUnsupported,
    TypedEffectExpression, TypedEffectKey, TypedEffectRelationship,
};
pub(crate) use effects::{
    CommandEffect, CommandEffectFallback, CommandEffects, EffectExpression, EffectFieldValue,
    EffectKey, EffectRelationship,
};
pub use outcomes::{
    Causal, CommandConsistency, CommandOutcome, PrepareCommandError, PreparedCommand, Projected,
    Succeeded,
};
pub(crate) use projection_obligations::{
    validate_projection_confirmation_count, CommandInputDefault, CommandProjectionConfirmation,
};
// Re-exported for unit tests that resolve obligations through this module path.
#[cfg_attr(not(test), allow(unused_imports))]
pub(crate) use projection_obligations::{
    ProjectionObligationResolutionError, ProjectorTopologyIdentity,
};
pub(crate) use projection_proof::{CommandCommitProofError, ProjectionCommitProof};
pub use typed_command::{typed_command, TypedCommand};
pub(crate) use typed_command::{TypedCommandContract, TypedServiceCommandBinding};
