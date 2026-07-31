//! Authoritative server interpreter for mutation programs.
//!
//! Resolves bound mutation IR through the existing projection physical
//! lowering path without exposing row-plan builders as authoring APIs.

use crate::projection::{
    ProjectionEventSelector, ProjectionExpression, ProjectionPartition, ProjectionProgram,
    ProjectionValueType, ResolvedProjectionPlan,
};
use crate::DomainEventOccurrence;

use super::bind::{MutationEventBinding, MutationInputBinding};
use super::program::MutationProgram;
use super::MutationProgramError;

/// Authoritative server-side mutation interpreter.
///
/// This adapter rewrites bound mutation programs for the existing projection mount
/// into the existing projection operation vocabulary and reuses projection
/// resolution. Direct and asynchronous commit lifecycles remain owned by the
/// projection/causal runtime.
#[derive(Clone, Debug)]
pub struct MutationServerInterpreter {
    binding: MutationEventBinding,
    projection_name: String,
    projection_version: u64,
    partition: ProjectionPartition,
    arm_id: String,
}

impl MutationServerInterpreter {
    /// Construct an interpreter for one portable event-to-mutation binding.
    pub fn new(
        binding: MutationEventBinding,
        projection_name: impl Into<String>,
        projection_version: u64,
        partition: ProjectionPartition,
        arm_id: impl Into<String>,
    ) -> Self {
        Self {
            binding,
            projection_name: projection_name.into(),
            projection_version,
            partition,
            arm_id: arm_id.into(),
        }
    }

    /// Return the underlying mutation program.
    pub fn program(&self) -> &MutationProgram {
        self.binding.program()
    }

    /// Return the event selector this interpreter handles.
    pub fn selector(&self) -> &ProjectionEventSelector {
        self.binding.selector()
    }

    /// Materialize the internal projection program.
    ///
    /// # Errors
    ///
    /// Propagates rewrite and projection validation failures.
    pub fn projection_program(&self) -> Result<ProjectionProgram, MutationProgramError> {
        self.binding.to_projection_program(
            self.projection_name.clone(),
            self.projection_version,
            self.partition.clone(),
            self.arm_id.clone(),
        )
    }

    /// Resolve one occurrence into a resolved projection plan via mutation IR.
    ///
    /// # Errors
    ///
    /// Rejects selector mismatches and rewrite/resolution failures.
    pub fn resolve(
        &self,
        occurrence: &DomainEventOccurrence,
    ) -> Result<ResolvedProjectionPlan, MutationProgramError> {
        if !self.binding.selector().matches(occurrence) {
            return Err(MutationProgramError::Adapter(
                "occurrence does not match mutation event binding".to_owned(),
            ));
        }
        let program = self.projection_program()?;
        ResolvedProjectionPlan::resolve(&program, occurrence).map_err(Into::into)
    }
}

/// Apply a bound mutation program by rewriting input paths with a custom binder.
///
/// # Errors
///
/// Propagates rewrite failures.
pub fn rewrite_program_with_binder(
    program: &MutationProgram,
    bind_input_path: &dyn Fn(
        &[String],
        &ProjectionValueType,
    ) -> Result<ProjectionExpression, MutationProgramError>,
) -> Result<Vec<crate::projection::ProjectionOperation>, MutationProgramError> {
    program.rewrite_to_projection_operations(bind_input_path)
}

/// Convenience: build a single-field body-path binder map for tests and simple
/// portable handlers.
pub fn simple_body_bindings(
    pairs: &[(&[&str], &[&str], ProjectionValueType)],
) -> Result<Vec<MutationInputBinding>, MutationProgramError> {
    pairs
        .iter()
        .map(|(input, body, value_type)| {
            super::bind::body_field_binding(
                input.iter().copied(),
                body.iter().copied(),
                value_type.clone(),
            )
        })
        .collect()
}
