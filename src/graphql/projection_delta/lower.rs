use std::collections::BTreeMap;

use crate::projection::placement::{
    ProjectionBindingState, ProjectionExecutionClass, ProjectionPlacement,
};
use crate::{
    ProjectionInvalidation, ProjectionMutationKind, ProjectionRelationshipEffectKind,
    ProjectionValueRef, ResolvedProjectionKey, ResolvedProjectionMutation, ResolvedProjectionPlan,
    ResolvedProjectionValue,
};

use super::authorization::ProjectionAuthorization;
use super::canonical::{canonicalize_operations, canonicalize_recoveries};
use super::{
    AuthorizationTransition, DeltaField, DeltaValue, ProjectionDelta, ProjectionDeltaError,
    ProjectionDeltaIdentity, ProjectionDeltaMutation, ProjectionDeltaOccurrence,
    ProjectionDeltaOperation, ProjectionDeltaProjectionIdentity, ProjectionDeltaRecovery,
    ProjectionDeltaRecoveryTarget, ProjectionDeltaScope, ProjectionDeltaSurfaceIdentity,
    ProjectionDeltaVisibility, ProjectionMutationSource, PROJECTION_DELTA_WIRE_VERSION,
};

/// Exact eligible binding pins used while lowering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectionDeltaSource {
    pub program_id: String,
    pub binding_id: String,
    pub epoch: String,
    pub program_ir_version: u16,
    pub operation_semantics_version: u16,
    pub placement: ProjectionPlacement,
    pub execution_class: ProjectionExecutionClass,
    pub state: ProjectionBindingState,
}

impl ProjectionDeltaSource {
    pub(crate) fn wire_identity(&self) -> ProjectionDeltaProjectionIdentity {
        ProjectionDeltaProjectionIdentity {
            program_id: self.program_id.clone(),
            binding_id: self.binding_id.clone(),
            epoch: self.epoch.clone(),
            program_ir_version: self.program_ir_version,
            operation_semantics_version: self.operation_semantics_version,
        }
    }

    fn validate_for(&self, source: ProjectionMutationSource) -> Result<(), ProjectionDeltaError> {
        if self.placement != ProjectionPlacement::Eventual
            || self.execution_class != ProjectionExecutionClass::Causal
        {
            return Err(ProjectionDeltaError::IneligibleBinding);
        }
        if self.state == ProjectionBindingState::Draining
            && source == ProjectionMutationSource::Preview
        {
            return Err(ProjectionDeltaError::IneligibleBinding);
        }
        self.wire_identity().validate()
    }
}

/// Command-wide role/application identity for lowering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectionDeltaContext {
    pub surface: ProjectionDeltaSurfaceIdentity,
    pub schema_fingerprint: String,
    pub authorization_generation: String,
    pub command_causation_id: String,
}

impl ProjectionDeltaContext {
    fn wire_identity(&self) -> ProjectionDeltaIdentity {
        ProjectionDeltaIdentity {
            surface: self.surface.clone(),
            schema_fingerprint: self.schema_fingerprint.clone(),
            authorization_generation: self.authorization_generation.clone(),
            command_causation_id: self.command_causation_id.clone(),
        }
    }
}

/// One server-resolved or compiler-preview logical mutation.
#[derive(Clone, Debug)]
pub struct LogicalProjectionMutation<'a> {
    pub source: ProjectionMutationSource,
    pub mutation: &'a ResolvedProjectionMutation,
    pub transition: AuthorizationTransition,
}

/// One explicit logical relationship invalidation independent of row mutation.
#[derive(Clone, Debug)]
pub struct LogicalProjectionInvalidation {
    pub invalidation: ProjectionInvalidation,
    pub source_key: Option<ProjectionDeltaScope>,
}

/// One relationship effect exposed for callers constructing preview batches.
#[derive(Clone, Debug)]
pub struct LogicalRelationshipEffect<'a> {
    pub source_model: &'a str,
    pub relationship: &'a str,
    pub target_model: &'a str,
    pub source_key: Option<&'a ResolvedProjectionKey>,
    pub target_key: Option<&'a ResolvedProjectionKey>,
    pub link: Option<bool>,
    pub transition: AuthorizationTransition,
}

/// One concrete/absent/unset logical field used by preview adapters.
#[derive(Clone, Debug)]
pub struct LogicalFieldValue {
    pub logical_field: String,
    pub value: ResolvedProjectionValue,
}

/// One zero-based command occurrence and all selected program plans.
#[derive(Clone, Debug)]
pub struct LogicalProjectionOccurrence<'a> {
    pub occurrence_id: String,
    pub plans: Vec<(&'a ProjectionDeltaSource, &'a ResolvedProjectionPlan)>,
    pub mutations: Vec<(&'a ProjectionDeltaSource, LogicalProjectionMutation<'a>)>,
}

/// Complete ordered logical input for one command delta.
#[derive(Clone, Debug)]
pub struct LogicalProjectionBatch<'a> {
    pub context: ProjectionDeltaContext,
    pub occurrences: Vec<LogicalProjectionOccurrence<'a>>,
}

/// Lower selected logical projection plans into one role-safe delta.
///
/// # Errors
///
/// Rejects ineligible/direct/draining bindings, identity mismatches,
/// authorization generation changes, unsafe mappings, and resource overflows.
pub fn lower_projection_delta(
    batch: &LogicalProjectionBatch<'_>,
    authorization: &impl ProjectionAuthorization,
) -> Result<ProjectionDelta, ProjectionDeltaError> {
    let identity = batch.context.wire_identity();
    identity.validate()?;
    let mut sources = batch
        .occurrences
        .iter()
        .flat_map(|occurrence| {
            occurrence
                .plans
                .iter()
                .map(|(source, _)| *source)
                .chain(occurrence.mutations.iter().map(|(source, _)| *source))
        })
        .collect::<Vec<_>>();
    sources.sort_by_key(|source| source.wire_identity());
    sources.dedup_by_key(|source| source.wire_identity());
    let projections = sources
        .iter()
        .map(|source| source.wire_identity())
        .collect::<Vec<_>>();
    let projection_indexes = projections
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, identity)| (identity, index as u32))
        .collect::<BTreeMap<_, _>>();

    let mut occurrences = Vec::with_capacity(batch.occurrences.len());
    let mut operations = Vec::new();
    let mut recoveries = Vec::new();
    for (ordinal, occurrence) in batch.occurrences.iter().enumerate() {
        occurrences.push(ProjectionDeltaOccurrence {
            causation_id: batch.context.command_causation_id.clone(),
            ordinal: ordinal as u32,
            occurrence_id: occurrence.occurrence_id.clone(),
        });
        for (source, plan) in &occurrence.plans {
            source.validate_for(ProjectionMutationSource::Actual)?;
            if plan.program_id().to_string() != source.program_id {
                return Err(ProjectionDeltaError::ProjectionIdentityMismatch);
            }
            let projection_ref = projection_indexes[&source.wire_identity()];
            for mutation in plan.mutations() {
                lower_mutation(
                    ordinal as u32,
                    projection_ref,
                    ProjectionMutationSource::Actual,
                    mutation,
                    AuthorizationTransition {
                        before: ProjectionDeltaVisibility::VisibleLive,
                        after: ProjectionDeltaVisibility::VisibleLive,
                    },
                    authorization,
                    &mut operations,
                    &mut recoveries,
                )?;
            }
        }
        for (source, logical) in &occurrence.mutations {
            source.validate_for(logical.source)?;
            let projection_ref = projection_indexes[&source.wire_identity()];
            lower_mutation(
                ordinal as u32,
                projection_ref,
                logical.source,
                logical.mutation,
                logical.transition,
                authorization,
                &mut operations,
                &mut recoveries,
            )?;
        }
    }
    let delta = ProjectionDelta {
        wire_version: PROJECTION_DELTA_WIRE_VERSION,
        identity,
        projections,
        occurrences,
        operations: canonicalize_operations(operations)?,
        recoveries: canonicalize_recoveries(recoveries),
    };
    delta.validate()?;
    Ok(delta)
}

#[expect(
    clippy::too_many_arguments,
    reason = "lowering keeps occurrence, source, authorization, and output accumulators explicit"
)]
fn lower_mutation(
    occurrence_ordinal: u32,
    projection_ref: u32,
    source: ProjectionMutationSource,
    mutation: &ResolvedProjectionMutation,
    transition: AuthorizationTransition,
    authorization: &impl ProjectionAuthorization,
    operations: &mut Vec<ProjectionDeltaOperation>,
    recoveries: &mut Vec<ProjectionDeltaRecovery>,
) -> Result<(), ProjectionDeltaError> {
    let Some(model) = authorization.model(mutation.target().model()) else {
        return Ok(());
    };
    let partition = authorization.partition(mutation.scope().partition())?;
    let key = authorization.record_key(mutation.target().model(), mutation.key())?;
    let scope = match (partition.clone(), key) {
        (Some(partition), Some(key)) => Some(ProjectionDeltaScope {
            partition,
            model: key.wire_model,
            key: key.fields,
        }),
        _ => None,
    };
    match (transition.before, transition.after) {
        (ProjectionDeltaVisibility::VisibleLive, ProjectionDeltaVisibility::VisibleLive) => {}
        (ProjectionDeltaVisibility::Hidden, ProjectionDeltaVisibility::Hidden) => return Ok(()),
        (ProjectionDeltaVisibility::VisibleLive, ProjectionDeltaVisibility::Hidden)
        | (ProjectionDeltaVisibility::Hidden, ProjectionDeltaVisibility::VisibleLive)
        | (ProjectionDeltaVisibility::Unknown, _)
        | (_, ProjectionDeltaVisibility::Unknown) => {
            recoveries.push(recovery(
                occurrence_ordinal,
                projection_ref,
                scope
                    .map(|scope| ProjectionDeltaRecoveryTarget::Record { scope })
                    .unwrap_or(ProjectionDeltaRecoveryTarget::Model {
                        partition: partition.clone(),
                        model: model.wire_model,
                    }),
            ));
            return Ok(());
        }
    }
    let Some(scope) = scope else {
        recoveries.push(recovery(
            occurrence_ordinal,
            projection_ref,
            ProjectionDeltaRecoveryTarget::Model {
                partition: partition.clone(),
                model: model.wire_model,
            },
        ));
        return Ok(());
    };
    let mut set = Vec::new();
    let mut unset = Vec::new();
    let mut unknown = false;
    for field in mutation.fields() {
        let Some(mapped) = authorization.field(mutation.target().model(), field.name()) else {
            continue;
        };
        match field.value() {
            ResolvedProjectionValue::Value(value) => set.push(DeltaField {
                field: mapped.wire_field,
                value: delta_value(value.as_ref())?,
            }),
            ResolvedProjectionValue::Absent => unknown = true,
            ResolvedProjectionValue::Unset => unset.push(mapped.wire_field),
        }
    }
    set.sort_by(|left, right| left.field.cmp(&right.field));
    unset.sort();
    let mutation_op = match mutation.kind() {
        ProjectionMutationKind::Delete => ProjectionDeltaMutation::Delete {
            scope: scope.clone(),
        },
        kind if complete_write(kind) && source == ProjectionMutationSource::Actual && !unknown => {
            ProjectionDeltaMutation::Upsert {
                scope: scope.clone(),
                fields: set,
                replace: model.replacement_fields,
            }
        }
        _ if !set.is_empty() || !unset.is_empty() => ProjectionDeltaMutation::Patch {
            scope: scope.clone(),
            set,
            unset,
            if_present: true,
        },
        _ => {
            recoveries.push(recovery(
                occurrence_ordinal,
                projection_ref,
                ProjectionDeltaRecoveryTarget::Record {
                    scope: scope.clone(),
                },
            ));
            lower_relationships(
                occurrence_ordinal,
                projection_ref,
                mutation,
                authorization,
                operations,
                recoveries,
            )?;
            return Ok(());
        }
    };
    operations.push(ProjectionDeltaOperation {
        occurrence_ordinal,
        projection_refs: vec![projection_ref],
        mutation: mutation_op,
    });
    lower_relationships(
        occurrence_ordinal,
        projection_ref,
        mutation,
        authorization,
        operations,
        recoveries,
    )
}

fn complete_write(kind: ProjectionMutationKind) -> bool {
    matches!(
        kind,
        ProjectionMutationKind::Insert
            | ProjectionMutationKind::Upsert
            | ProjectionMutationKind::Recreate
            | ProjectionMutationKind::InsertRelated
            | ProjectionMutationKind::UpsertRelated
    )
}

fn lower_relationships(
    occurrence_ordinal: u32,
    projection_ref: u32,
    mutation: &ResolvedProjectionMutation,
    authorization: &impl ProjectionAuthorization,
    operations: &mut Vec<ProjectionDeltaOperation>,
    recoveries: &mut Vec<ProjectionDeltaRecovery>,
) -> Result<(), ProjectionDeltaError> {
    for effect in mutation.provenance().relationship_effects() {
        let relationship = effect.relationship();
        let Some(mapped) = authorization.relationship(
            relationship.source_model(),
            relationship.relationship(),
            relationship.target_model(),
        ) else {
            continue;
        };
        let source = effect
            .source_key()
            .map(|key| {
                authorized_scope(
                    mutation.scope().partition(),
                    relationship.source_model(),
                    key,
                    authorization,
                )
            })
            .transpose()?
            .flatten();
        let target = effect
            .target_key()
            .map(|key| {
                authorized_scope(
                    mutation.scope().partition(),
                    relationship.target_model(),
                    key,
                    authorization,
                )
            })
            .transpose()?
            .flatten();
        match (effect.kind(), source, target) {
            (ProjectionRelationshipEffectKind::Link, Some(source), Some(target)) => {
                operations.push(ProjectionDeltaOperation {
                    occurrence_ordinal,
                    projection_refs: vec![projection_ref],
                    mutation: ProjectionDeltaMutation::Link {
                        relationship: mapped.wire_relationship,
                        source,
                        target,
                    },
                });
            }
            (ProjectionRelationshipEffectKind::Unlink, Some(source), Some(target)) => {
                operations.push(ProjectionDeltaOperation {
                    occurrence_ordinal,
                    projection_refs: vec![projection_ref],
                    mutation: ProjectionDeltaMutation::Unlink {
                        relationship: mapped.wire_relationship,
                        source,
                        target,
                    },
                });
            }
            (_, Some(source), _) => {
                operations.push(ProjectionDeltaOperation {
                    occurrence_ordinal,
                    projection_refs: vec![projection_ref],
                    mutation: ProjectionDeltaMutation::InvalidateRelationship {
                        relationship: mapped.wire_relationship.clone(),
                        source: source.clone(),
                    },
                });
                recoveries.push(recovery(
                    occurrence_ordinal,
                    projection_ref,
                    ProjectionDeltaRecoveryTarget::Relationship {
                        relationship: mapped.wire_relationship,
                        source,
                    },
                ));
            }
            _ => {
                if let Some(model) = authorization.model(relationship.source_model()) {
                    operations.push(ProjectionDeltaOperation {
                        occurrence_ordinal,
                        projection_refs: vec![projection_ref],
                        mutation: ProjectionDeltaMutation::InvalidateModel {
                            partition: authorization.partition(mutation.scope().partition())?,
                            model: model.wire_model.clone(),
                        },
                    });
                    recoveries.push(recovery(
                        occurrence_ordinal,
                        projection_ref,
                        ProjectionDeltaRecoveryTarget::Model {
                            partition: authorization.partition(mutation.scope().partition())?,
                            model: model.wire_model,
                        },
                    ));
                }
            }
        }
    }
    for invalidation in mutation.provenance().invalidations() {
        if let ProjectionInvalidation::Model { model } = invalidation {
            if let Some(model) = authorization.model(model) {
                operations.push(ProjectionDeltaOperation {
                    occurrence_ordinal,
                    projection_refs: vec![projection_ref],
                    mutation: ProjectionDeltaMutation::InvalidateModel {
                        partition: authorization.partition(mutation.scope().partition())?,
                        model: model.wire_model.clone(),
                    },
                });
                recoveries.push(recovery(
                    occurrence_ordinal,
                    projection_ref,
                    ProjectionDeltaRecoveryTarget::Model {
                        partition: authorization.partition(mutation.scope().partition())?,
                        model: model.wire_model,
                    },
                ));
            }
        }
    }
    Ok(())
}

fn authorized_scope(
    partition: &crate::ResolvedProjectionPartition,
    logical_model: &str,
    key: &ResolvedProjectionKey,
    authorization: &impl ProjectionAuthorization,
) -> Result<Option<ProjectionDeltaScope>, ProjectionDeltaError> {
    let partition = authorization.partition(partition)?;
    let key = authorization.record_key(logical_model, key)?;
    Ok(match (partition, key) {
        (Some(partition), Some(key)) => Some(ProjectionDeltaScope {
            partition,
            model: key.wire_model,
            key: key.fields,
        }),
        _ => None,
    })
}

fn recovery(
    occurrence_ordinal: u32,
    projection_ref: u32,
    target: ProjectionDeltaRecoveryTarget,
) -> ProjectionDeltaRecovery {
    ProjectionDeltaRecovery {
        occurrence_ordinal,
        projection_refs: vec![projection_ref],
        target,
    }
}

fn delta_value(value: ProjectionValueRef<'_>) -> Result<DeltaValue, ProjectionDeltaError> {
    let value = match value {
        ProjectionValueRef::Null => DeltaValue::Null,
        ProjectionValueRef::Boolean(value) => DeltaValue::Boolean(value),
        ProjectionValueRef::I64(value) => DeltaValue::I64(value.to_owned()),
        ProjectionValueRef::U64(value) => DeltaValue::U64(value.to_owned()),
        ProjectionValueRef::F64(value) => DeltaValue::F64(value.to_owned()),
        ProjectionValueRef::String(value) => DeltaValue::String(value.to_owned()),
        ProjectionValueRef::Enum { enum_type, variant } => DeltaValue::Enum {
            enum_type: enum_type.to_owned(),
            variant: variant.to_owned(),
        },
        ProjectionValueRef::List(values) => DeltaValue::List(
            values
                .iter()
                .map(|value| delta_value(value.as_ref()))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        ProjectionValueRef::Object(fields) => DeltaValue::Object(
            fields
                .iter()
                .map(|field| {
                    Ok(DeltaField {
                        field: field.name().to_owned(),
                        value: delta_value(field.value().as_ref())?,
                    })
                })
                .collect::<Result<Vec<_>, ProjectionDeltaError>>()?,
        ),
    };
    value.validate()?;
    Ok(value)
}
