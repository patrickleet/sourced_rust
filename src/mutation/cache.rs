//! Role-safe cache lowering for mutation programs.
//!
//! Lowers the same logical mutation IR through Surface authorization and
//! replica coverage into the existing `ProjectionDelta` algebra. During the
//! this module rewrites mutations into projection operations for cache lowering
//! and reuses projection-delta lowering when available.

use serde::Serialize;

use crate::projection::{
    ProjectionInvalidation, ProjectionMutationKind, ProjectionOperation, ProjectionTarget,
};

use super::expression::{MutationAssignment, ResolvedMutationValue};
use super::program::{MutationKind, MutationOperation, MutationProgram};
use super::MutationProgramError;

/// Semantic cache consequence derived from one mutation operation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MutationCacheEffect {
    /// Upsert a complete normalized record.
    Upsert {
        /// Target model.
        target: ProjectionTarget,
        /// Ordered field names written.
        fields: Vec<String>,
    },
    /// Patch an existing visible base; never synthesizes a partial record.
    Patch {
        /// Target model.
        target: ProjectionTarget,
        /// Ordered field names patched.
        fields: Vec<String>,
    },
    /// Provisionally hide a record.
    Delete {
        /// Target model.
        target: ProjectionTarget,
    },
    /// Update a covered relationship index.
    Link {
        /// Target model that owns the relationship edge write.
        target: ProjectionTarget,
    },
    /// Remove a covered relationship index edge.
    Unlink {
        /// Target model that owns the relationship edge write.
        target: ProjectionTarget,
    },
    /// Mark the narrowest safe dependency stale.
    Invalidate {
        /// Invalidation inventory.
        invalidations: Vec<ProjectionInvalidation>,
    },
}

/// Atomic cache program derived from one mutation program.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MutationCacheProgram {
    /// Ordered effects applied atomically to one optimistic layer.
    effects: Vec<MutationCacheEffect>,
}

impl MutationCacheProgram {
    /// Return ordered cache effects.
    pub fn effects(&self) -> &[MutationCacheEffect] {
        &self.effects
    }
}

/// Lower a mutation program to role-safe cache effects.
///
/// Authorization and coverage gates are applied by the caller via
/// `visibility`. When visibility rejects an operation, the narrowest
/// invalidation is emitted instead of a concrete write.
///
/// # Errors
///
/// Returns validation failures for unsupported operation shapes.
pub fn lower_mutation_cache(
    program: &MutationProgram,
    visibility: &MutationCacheVisibility,
) -> Result<MutationCacheProgram, MutationProgramError> {
    let mut effects = Vec::with_capacity(program.operations().len());
    for operation in program.operations() {
        effects.push(lower_operation(operation, visibility)?);
    }
    Ok(MutationCacheProgram { effects })
}

/// Role/app authority and replica coverage facts for one lowering.
#[derive(Clone, Debug, Default)]
pub struct MutationCacheVisibility {
    /// When false, every concrete write collapses to model invalidation.
    pub authorized: bool,
    /// When false, patches cannot be applied locally and become invalidation.
    pub has_base_record: bool,
    /// When false, relationship effects become invalidation.
    pub relationship_covered: bool,
}

impl MutationCacheVisibility {
    /// Fully authorized, covered visibility for golden vectors.
    pub fn full() -> Self {
        Self {
            authorized: true,
            has_base_record: true,
            relationship_covered: true,
        }
    }

    /// Unauthorized principal: fail closed to invalidation.
    pub fn unauthorized() -> Self {
        Self {
            authorized: false,
            has_base_record: true,
            relationship_covered: true,
        }
    }
}

fn lower_operation(
    operation: &MutationOperation,
    visibility: &MutationCacheVisibility,
) -> Result<MutationCacheEffect, MutationProgramError> {
    if !visibility.authorized {
        return Ok(MutationCacheEffect::Invalidate {
            invalidations: model_invalidation(operation),
        });
    }
    match operation.kind() {
        MutationKind::Insert
        | MutationKind::Upsert
        | MutationKind::Recreate
        | MutationKind::InsertRelated
        | MutationKind::UpsertRelated => {
            let fields = concrete_field_names(operation);
            if fields.is_empty() {
                return Ok(MutationCacheEffect::Invalidate {
                    invalidations: model_invalidation(operation),
                });
            }
            Ok(MutationCacheEffect::Upsert {
                target: operation.target().clone(),
                fields,
            })
        }
        MutationKind::Patch | MutationKind::UpsertPatch => {
            if !visibility.has_base_record {
                // Partial patches never create records.
                return Ok(MutationCacheEffect::Invalidate {
                    invalidations: model_invalidation(operation),
                });
            }
            let fields = concrete_field_names(operation);
            if fields.is_empty() {
                return Ok(MutationCacheEffect::Invalidate {
                    invalidations: model_invalidation(operation),
                });
            }
            Ok(MutationCacheEffect::Patch {
                target: operation.target().clone(),
                fields,
            })
        }
        MutationKind::Delete => Ok(MutationCacheEffect::Delete {
            target: operation.target().clone(),
        }),
        MutationKind::Invalidate => Ok(MutationCacheEffect::Invalidate {
            invalidations: if operation.invalidations().is_empty() {
                model_invalidation(operation)
            } else {
                operation.invalidations().to_vec()
            },
        }),
    }
    .map(|effect| {
        // Relationship effects: when uncovered, prefer invalidation.
        if !visibility.relationship_covered && !operation.relationship_effects().is_empty() {
            MutationCacheEffect::Invalidate {
                invalidations: model_invalidation(operation),
            }
        } else if !operation.relationship_effects().is_empty() {
            // Prefer explicit link/unlink when the primary op is related.
            if operation.kind().is_related() {
                if operation.relationship_effects().iter().any(|effect| {
                    matches!(
                        effect.kind(),
                        crate::projection::ProjectionRelationshipEffectKind::Unlink
                    )
                }) {
                    MutationCacheEffect::Unlink {
                        target: operation.target().clone(),
                    }
                } else if operation.relationship_effects().iter().any(|effect| {
                    matches!(
                        effect.kind(),
                        crate::projection::ProjectionRelationshipEffectKind::Link
                    )
                }) {
                    MutationCacheEffect::Link {
                        target: operation.target().clone(),
                    }
                } else {
                    effect
                }
            } else {
                effect
            }
        } else {
            effect
        }
    })
}

fn concrete_field_names(operation: &MutationOperation) -> Vec<String> {
    operation
        .fields()
        .iter()
        .filter(|field| {
            matches!(
                field.assignment(),
                MutationAssignment::Set(_) | MutationAssignment::Unset
            )
        })
        .filter(|field| !matches!(field.assignment(), MutationAssignment::Unknown))
        .map(|field| field.name().to_owned())
        .collect()
}

fn model_invalidation(operation: &MutationOperation) -> Vec<ProjectionInvalidation> {
    if !operation.invalidations().is_empty() {
        return operation.invalidations().to_vec();
    }
    ProjectionInvalidation::model(operation.target().model().to_owned())
        .into_iter()
        .collect()
}

/// Lower rewritten projection operations into cache effects (shared vector path).
pub fn lower_projection_ops_cache(
    operations: &[ProjectionOperation],
    visibility: &MutationCacheVisibility,
) -> Result<MutationCacheProgram, MutationProgramError> {
    let mut effects = Vec::with_capacity(operations.len());
    for operation in operations {
        let kind = match operation.kind() {
            ProjectionMutationKind::Insert
            | ProjectionMutationKind::Upsert
            | ProjectionMutationKind::Recreate
            | ProjectionMutationKind::InsertRelated
            | ProjectionMutationKind::UpsertRelated => {
                if !visibility.authorized {
                    MutationCacheEffect::Invalidate {
                        invalidations: vec![ProjectionInvalidation::model(
                            operation.target().model(),
                        )
                        .map_err(MutationProgramError::from)?],
                    }
                } else {
                    MutationCacheEffect::Upsert {
                        target: operation.target().clone(),
                        fields: operation
                            .fields()
                            .iter()
                            .map(|field| field.name().to_owned())
                            .collect(),
                    }
                }
            }
            ProjectionMutationKind::Patch | ProjectionMutationKind::UpsertPatch => {
                if !visibility.authorized || !visibility.has_base_record {
                    MutationCacheEffect::Invalidate {
                        invalidations: vec![ProjectionInvalidation::model(
                            operation.target().model(),
                        )
                        .map_err(MutationProgramError::from)?],
                    }
                } else {
                    MutationCacheEffect::Patch {
                        target: operation.target().clone(),
                        fields: operation
                            .fields()
                            .iter()
                            .map(|field| field.name().to_owned())
                            .collect(),
                    }
                }
            }
            ProjectionMutationKind::Delete => {
                if !visibility.authorized {
                    MutationCacheEffect::Invalidate {
                        invalidations: vec![ProjectionInvalidation::model(
                            operation.target().model(),
                        )
                        .map_err(MutationProgramError::from)?],
                    }
                } else {
                    MutationCacheEffect::Delete {
                        target: operation.target().clone(),
                    }
                }
            }
        };
        effects.push(kind);
    }
    Ok(MutationCacheProgram { effects })
}

/// Evaluate whether a resolved mutation value is writable in cache.
pub fn is_cache_writable(value: &ResolvedMutationValue) -> bool {
    matches!(
        value,
        ResolvedMutationValue::Value(_) | ResolvedMutationValue::Unset
    )
}

/// Placeholder for shared golden-vector evaluation of field presence.
pub fn presence_label(value: &ResolvedMutationValue) -> &'static str {
    match value {
        ResolvedMutationValue::Value(value) if value.is_null() => "null",
        ResolvedMutationValue::Value(_) => "value",
        ResolvedMutationValue::Absent => "absent",
        ResolvedMutationValue::Unset => "unset",
        ResolvedMutationValue::Unknown => "unknown",
    }
}
