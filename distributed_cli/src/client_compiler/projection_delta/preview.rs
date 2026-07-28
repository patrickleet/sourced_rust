use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use super::super::manifest::*;
use super::super::ClientCompileError;
use super::PROJECTION_DELTA_WIRE_VERSION;

const PREVIEW_PLAN_VERSION: u16 = 1;

/// Compiler knowledge is deliberately richer than JavaScript values.
///
/// `Known(Null)` is a value. `Unset` is an explicit absence. `Denied` and
/// `CacheUnowned` are authorization/coverage states and never become artifact
/// paths or values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Knowledge {
    Known(PreviewExpression),
    Unknown,
    Unset,
    Denied,
    #[allow(
        dead_code,
        reason = "reserved lattice state supplied by the Task 16 replica capability check"
    )]
    CacheUnowned,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum PreviewExpression {
    Input {
        path: Vec<String>,
    },
    GeneratedDefault {
        path: Vec<String>,
    },
    TrustedPreset {
        name: String,
        codec: String,
    },
    Constant {
        value: ManifestProjectionValue,
    },
    Null,
    List {
        values: Vec<PreviewExpression>,
    },
    Object {
        fields: Vec<PreviewObjectField>,
    },
    Transform {
        transform: ManifestProjectionScalarTransform,
        arguments: Vec<PreviewExpression>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct PreviewObjectField {
    name: String,
    value: PreviewExpression,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompiledCommandProjection {
    version: u32,
    delta_wire_version: u16,
    projection_program_version: u32,
    operation_semantics_version: u16,
    projections: Vec<PreviewProjectionIdentity>,
    event_set: Vec<ManifestProjectionEventRef>,
    preview: PreviewPlan,
    fallback: ManifestProjectionFallback,
}

impl CompiledCommandProjection {
    pub(crate) fn affected_models(&self) -> BTreeSet<String> {
        let mut models = BTreeSet::new();
        for operation in &self.preview.operations {
            operation.mutation.collect_models(&mut models);
        }
        for recovery in &self.preview.recoveries {
            recovery.target.collect_models(&mut models);
        }
        models
    }

    pub(crate) fn affected_relationships(&self) -> BTreeSet<(String, String, String)> {
        let mut relationships = BTreeSet::new();
        for operation in &self.preview.operations {
            operation.mutation.collect_relationships(&mut relationships);
        }
        relationships
    }

    pub(crate) fn requires_revalidation(&self) -> bool {
        !self.preview.recoveries.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
struct PreviewProjectionIdentity {
    program_id: String,
    binding_id: String,
    epoch: String,
    program_ir_version: u16,
    operation_semantics_version: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct PreviewPlan {
    version: u16,
    occurrences: Vec<PreviewOccurrence>,
    operations: Vec<PreviewOperation>,
    recoveries: Vec<PreviewRecovery>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct PreviewOccurrence {
    ordinal: u32,
    event: ManifestProjectionEventRef,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct PreviewOperation {
    occurrence_ordinal: u32,
    projection_refs: Vec<u32>,
    mutation: PreviewMutation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
struct PreviewScope {
    partition: PreviewPartition,
    model: String,
    key: Vec<PreviewKeyField>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum PreviewPartition {
    Unit,
    Expression {
        expression: PreviewExpression,
        requires: PreviewPartitionRequirement,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum PreviewPartitionRequirement {
    CurrentCachePartition,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct PreviewKeyField {
    ordinal: u32,
    field: String,
    value: PreviewExpression,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct PreviewField {
    field: String,
    value: PreviewExpression,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum PreviewMutation {
    Upsert {
        scope: PreviewScope,
        fields: Vec<PreviewField>,
        replace: Vec<String>,
    },
    Patch {
        scope: PreviewScope,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        set: Vec<PreviewField>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        unset: Vec<String>,
        if_present: bool,
    },
    Delete {
        scope: PreviewScope,
    },
    Link {
        relationship: String,
        source: PreviewScope,
        target: PreviewScope,
    },
    Unlink {
        relationship: String,
        source: PreviewScope,
        target: PreviewScope,
    },
    InvalidateModel {
        #[serde(skip_serializing_if = "Option::is_none")]
        partition: Option<PreviewPartition>,
        model: String,
    },
    InvalidateRelationship {
        relationship: String,
        source: PreviewScope,
    },
}

impl PreviewMutation {
    fn canonical_scope(&self) -> Result<PreviewOperationScope, ClientCompileError> {
        let scope_json = |scope: &PreviewScope| {
            serde_json::to_string(scope).map_err(|error| {
                ClientCompileError::manifest(
                    "client.projection_preview.canonical",
                    format!("cannot canonicalize preview scope: {error}"),
                )
            })
        };
        Ok(match self {
            Self::Upsert { scope, .. } | Self::Patch { scope, .. } | Self::Delete { scope } => {
                PreviewOperationScope::Record(scope_json(scope)?)
            }
            Self::Link {
                relationship,
                source,
                target,
            }
            | Self::Unlink {
                relationship,
                source,
                target,
            } => PreviewOperationScope::Edge {
                relationship: relationship.clone(),
                source: scope_json(source)?,
                target: scope_json(target)?,
            },
            Self::InvalidateModel { partition, model } => PreviewOperationScope::Model {
                partition: partition
                    .as_ref()
                    .map(|partition| serde_json::to_string(partition))
                    .transpose()
                    .map_err(|error| {
                        ClientCompileError::manifest(
                            "client.projection_preview.canonical",
                            format!("cannot canonicalize preview partition: {error}"),
                        )
                    })?,
                model: model.clone(),
            },
            Self::InvalidateRelationship {
                relationship,
                source,
            } => PreviewOperationScope::Relationship {
                relationship: relationship.clone(),
                source: scope_json(source)?,
            },
        })
    }

    fn collect_models(&self, models: &mut BTreeSet<String>) {
        match self {
            Self::Upsert { scope, .. } | Self::Patch { scope, .. } | Self::Delete { scope } => {
                models.insert(scope.model.clone());
            }
            Self::Link { source, target, .. } | Self::Unlink { source, target, .. } => {
                models.insert(source.model.clone());
                models.insert(target.model.clone());
            }
            Self::InvalidateModel { model, .. } => {
                models.insert(model.clone());
            }
            Self::InvalidateRelationship { source, .. } => {
                models.insert(source.model.clone());
            }
        }
    }

    fn collect_relationships(&self, relationships: &mut BTreeSet<(String, String, String)>) {
        match self {
            Self::Link {
                relationship,
                source,
                target,
            }
            | Self::Unlink {
                relationship,
                source,
                target,
            } => {
                relationships.insert((
                    source.model.clone(),
                    relationship.clone(),
                    target.model.clone(),
                ));
            }
            Self::InvalidateRelationship {
                relationship,
                source,
            } => {
                relationships.insert((source.model.clone(), relationship.clone(), String::new()));
            }
            _ => {}
        }
    }
}

/// The variant declaration order is the canonical operation-kind order shared
/// with the authoritative ProjectionDelta wire.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum PreviewOperationScope {
    Record(String),
    Edge {
        relationship: String,
        source: String,
        target: String,
    },
    Model {
        partition: Option<String>,
        model: String,
    },
    Relationship {
        relationship: String,
        source: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct PreviewRecovery {
    occurrence_ordinal: u32,
    projection_refs: Vec<u32>,
    condition: PreviewRecoveryCondition,
    target: PreviewRecoveryTarget,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
enum PreviewRecoveryCondition {
    Always,
    IfRecordMissing,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum PreviewRecoveryTarget {
    Record {
        scope: PreviewScope,
    },
    Relationship {
        relationship: String,
        source: PreviewScope,
    },
    Model {
        #[serde(skip_serializing_if = "Option::is_none")]
        partition: Option<PreviewPartition>,
        model: String,
    },
}

impl PreviewRecoveryTarget {
    fn canonical_key(&self) -> Result<PreviewRecoveryTargetKey, ClientCompileError> {
        let scope_json = |scope: &PreviewScope| {
            serde_json::to_string(scope).map_err(|error| {
                ClientCompileError::manifest(
                    "client.projection_preview.canonical",
                    format!("cannot canonicalize preview recovery scope: {error}"),
                )
            })
        };
        Ok(match self {
            Self::Record { scope } => PreviewRecoveryTargetKey::Record(scope_json(scope)?),
            Self::Relationship {
                relationship,
                source,
            } => PreviewRecoveryTargetKey::Relationship {
                relationship: relationship.clone(),
                source: scope_json(source)?,
            },
            Self::Model { partition, model } => PreviewRecoveryTargetKey::Model {
                partition: partition
                    .as_ref()
                    .map(|partition| serde_json::to_string(partition))
                    .transpose()
                    .map_err(|error| {
                        ClientCompileError::manifest(
                            "client.projection_preview.canonical",
                            format!("cannot canonicalize preview recovery partition: {error}"),
                        )
                    })?,
                model: model.clone(),
            },
        })
    }

    fn collect_models(&self, models: &mut BTreeSet<String>) {
        match self {
            Self::Record { scope } | Self::Relationship { source: scope, .. } => {
                models.insert(scope.model.clone());
            }
            Self::Model { model, .. } => {
                models.insert(model.clone());
            }
        }
    }
}

/// The variant declaration order is the canonical recovery-kind order shared
/// with the authoritative ProjectionDelta wire.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum PreviewRecoveryTargetKey {
    Record(String),
    Relationship {
        relationship: String,
        source: String,
    },
    Model {
        partition: Option<String>,
        model: String,
    },
}

pub(crate) fn compile_command_preview(
    command: &ManifestCommand,
    manifest: &ClientManifest,
) -> Result<Option<CompiledCommandProjection>, ClientCompileError> {
    let Some(extension) = &command.extensions.projection else {
        return Ok(None);
    };
    let programs = manifest
        .projection_programs
        .iter()
        .map(|program| (program.program_id.as_str(), program))
        .collect::<BTreeMap<_, _>>();
    let selected_program_ids = extension
        .program_arms
        .iter()
        .map(|selected| selected.program_id.as_str())
        .collect::<BTreeSet<_>>();
    let mut identities = Vec::new();
    for program_id in selected_program_ids {
        let program = programs
            .get(program_id)
            .expect("manifest validation proved selected program");
        let binding = manifest
            .projection_bindings
            .iter()
            .find(|binding| {
                binding.program_id == program_id
                    && binding.state == ManifestProjectionBindingState::Active
                    && binding.placement == ManifestProjectionPlacement::Eventual
                    && binding.execution_class == ManifestProjectionExecutionClass::Causal
            })
            .expect("manifest validation proved one eligible binding");
        identities.push(PreviewProjectionIdentity {
            program_id: program.program_id.clone(),
            binding_id: binding.binding_id.clone(),
            epoch: binding.epoch.clone(),
            program_ir_version: program.ir_version,
            operation_semantics_version: program.operation_semantics_version,
        });
    }
    identities.sort();
    let projection_refs = identities
        .iter()
        .enumerate()
        .map(|(index, identity)| (identity.program_id.as_str(), index as u32))
        .collect::<BTreeMap<_, _>>();

    let mut occurrences = Vec::new();
    let mut operations = Vec::new();
    let mut recoveries = Vec::new();
    for occurrence in &extension.preview_occurrences {
        occurrences.push(PreviewOccurrence {
            ordinal: occurrence.ordinal,
            event: occurrence.event.clone(),
        });
        let slots = occurrence
            .values
            .iter()
            .map(|value| (value.slot.as_str(), source_knowledge(&value.source)))
            .collect::<BTreeMap<_, _>>();
        for selected in extension
            .program_arms
            .iter()
            .filter(|selected| selected.event == occurrence.event)
        {
            let program = programs[selected.program_id.as_str()];
            let arm = program
                .arms
                .iter()
                .find(|arm| arm.arm == selected.arm && arm.event == selected.event)
                .expect("manifest validation proved selected arm");
            let projection_ref = projection_refs[selected.program_id.as_str()];
            lower_arm(
                occurrence.ordinal,
                projection_ref,
                &occurrence.event,
                arm,
                &slots,
                manifest,
                &mut operations,
                &mut recoveries,
            )?;
        }
    }
    let operations = canonicalize_operations(operations)?;
    let recoveries = canonicalize_recoveries(recoveries, &operations)?;
    Ok(Some(CompiledCommandProjection {
        version: extension.version,
        delta_wire_version: PROJECTION_DELTA_WIRE_VERSION,
        projection_program_version: CLIENT_PROJECTION_PROGRAM_VERSION,
        operation_semantics_version: PROJECTION_OPERATION_SEMANTICS_VERSION,
        projections: identities,
        event_set: extension.event_set.clone(),
        preview: PreviewPlan {
            version: PREVIEW_PLAN_VERSION,
            occurrences,
            operations,
            recoveries,
        },
        fallback: extension.fallback,
    }))
}

#[allow(clippy::too_many_arguments)]
fn lower_arm(
    occurrence_ordinal: u32,
    projection_ref: u32,
    event: &ManifestProjectionEventRef,
    arm: &ManifestProjectionArm,
    slots: &BTreeMap<&str, Knowledge>,
    manifest: &ClientManifest,
    operations: &mut Vec<PreviewOperation>,
    recoveries: &mut Vec<PreviewRecovery>,
) -> Result<(), ClientCompileError> {
    let partition = evaluate_partition(&arm.partition, slots, event);
    for operation in &arm.operations {
        lower_record(
            occurrence_ordinal,
            projection_ref,
            event,
            operation,
            &partition,
            slots,
            manifest,
            operations,
            recoveries,
        )?;
        lower_relationships(
            occurrence_ordinal,
            projection_ref,
            event,
            operation,
            &partition,
            slots,
            manifest,
            operations,
            recoveries,
        )?;
        lower_invalidations(
            occurrence_ordinal,
            projection_ref,
            event,
            operation,
            &partition,
            slots,
            manifest,
            operations,
            recoveries,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn lower_record(
    occurrence_ordinal: u32,
    projection_ref: u32,
    event: &ManifestProjectionEventRef,
    operation: &ManifestProjectionOperation,
    partition: &Option<PreviewPartition>,
    slots: &BTreeMap<&str, Knowledge>,
    manifest: &ClientManifest,
    operations: &mut Vec<PreviewOperation>,
    recoveries: &mut Vec<PreviewRecovery>,
) -> Result<(), ClientCompileError> {
    if matches!(
        operation.kind,
        ManifestProjectionMutationKind::InvalidateModel
            | ManifestProjectionMutationKind::InvalidateRelationship
    ) {
        return Ok(());
    }
    let partition = partition.clone();
    let key = evaluate_key(&operation.key, slots, event);
    let Some(scope) = partition
        .clone()
        .zip(key)
        .map(|(partition, key)| PreviewScope {
            partition,
            model: operation.model.clone(),
            key,
        })
    else {
        recoveries.push(model_recovery(
            occurrence_ordinal,
            projection_ref,
            operation.model.clone(),
            partition,
        ));
        return Ok(());
    };
    let model = manifest
        .models
        .get(&operation.model)
        .expect("manifest validation proved operation model");
    let replacement_fields = model
        .fields
        .iter()
        .filter(|field| {
            model
                .identity()
                .is_none_or(|identity| identity.iter().all(|identity| identity.name != field.name))
        })
        .map(|field| field.name.clone())
        .collect::<Vec<_>>();
    let (set, unset, uncertain) = evaluate_fields(&operation.fields, slots, event);
    match operation.kind {
        ManifestProjectionMutationKind::Insert
        | ManifestProjectionMutationKind::Upsert
        | ManifestProjectionMutationKind::Recreate
        | ManifestProjectionMutationKind::InsertRelated
        | ManifestProjectionMutationKind::UpsertRelated => {
            let mapped = set
                .iter()
                .map(|field| field.field.as_str())
                .chain(unset.iter().map(String::as_str))
                .collect::<BTreeSet<_>>();
            let complete = !uncertain
                && replacement_fields
                    .iter()
                    .map(String::as_str)
                    .collect::<BTreeSet<_>>()
                    == mapped;
            if complete {
                operations.push(PreviewOperation {
                    occurrence_ordinal,
                    projection_refs: vec![projection_ref],
                    mutation: PreviewMutation::Upsert {
                        scope,
                        fields: set,
                        replace: replacement_fields,
                    },
                });
            } else if !set.is_empty() || !unset.is_empty() {
                operations.push(PreviewOperation {
                    occurrence_ordinal,
                    projection_refs: vec![projection_ref],
                    mutation: PreviewMutation::Patch {
                        scope: scope.clone(),
                        set,
                        unset,
                        if_present: true,
                    },
                });
                recoveries.push(record_recovery(
                    occurrence_ordinal,
                    projection_ref,
                    scope,
                    PreviewRecoveryCondition::IfRecordMissing,
                ));
            } else {
                recoveries.push(record_recovery(
                    occurrence_ordinal,
                    projection_ref,
                    scope,
                    PreviewRecoveryCondition::Always,
                ));
            }
        }
        ManifestProjectionMutationKind::Patch | ManifestProjectionMutationKind::UpsertPatch => {
            if !set.is_empty() || !unset.is_empty() {
                operations.push(PreviewOperation {
                    occurrence_ordinal,
                    projection_refs: vec![projection_ref],
                    mutation: PreviewMutation::Patch {
                        scope: scope.clone(),
                        set,
                        unset,
                        if_present: true,
                    },
                });
                recoveries.push(record_recovery(
                    occurrence_ordinal,
                    projection_ref,
                    scope,
                    PreviewRecoveryCondition::IfRecordMissing,
                ));
            } else {
                recoveries.push(record_recovery(
                    occurrence_ordinal,
                    projection_ref,
                    scope,
                    PreviewRecoveryCondition::Always,
                ));
            }
        }
        ManifestProjectionMutationKind::Delete => operations.push(PreviewOperation {
            occurrence_ordinal,
            projection_refs: vec![projection_ref],
            mutation: PreviewMutation::Delete { scope },
        }),
        ManifestProjectionMutationKind::InvalidateModel
        | ManifestProjectionMutationKind::InvalidateRelationship => unreachable!(),
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn lower_relationships(
    occurrence_ordinal: u32,
    projection_ref: u32,
    event: &ManifestProjectionEventRef,
    operation: &ManifestProjectionOperation,
    partition: &Option<PreviewPartition>,
    slots: &BTreeMap<&str, Knowledge>,
    _manifest: &ClientManifest,
    operations: &mut Vec<PreviewOperation>,
    recoveries: &mut Vec<PreviewRecovery>,
) -> Result<(), ClientCompileError> {
    let partition = partition.clone();
    for effect in &operation.relationships {
        let source = partition
            .clone()
            .zip(evaluate_key(&effect.source_key, slots, event))
            .map(|(partition, key)| PreviewScope {
                partition,
                model: effect.source_model.clone(),
                key,
            });
        let target = partition
            .clone()
            .zip(evaluate_key(&effect.target_key, slots, event))
            .map(|(partition, key)| PreviewScope {
                partition,
                model: effect.target_model.clone(),
                key,
            });
        match (effect.kind, source, target) {
            (ManifestProjectionRelationshipEffectKind::Link, Some(source), Some(target)) => {
                operations.push(PreviewOperation {
                    occurrence_ordinal,
                    projection_refs: vec![projection_ref],
                    mutation: PreviewMutation::Link {
                        relationship: effect.relationship.clone(),
                        source,
                        target,
                    },
                });
            }
            (ManifestProjectionRelationshipEffectKind::Unlink, Some(source), Some(target)) => {
                operations.push(PreviewOperation {
                    occurrence_ordinal,
                    projection_refs: vec![projection_ref],
                    mutation: PreviewMutation::Unlink {
                        relationship: effect.relationship.clone(),
                        source,
                        target,
                    },
                });
            }
            (_, Some(source), _) => {
                operations.push(PreviewOperation {
                    occurrence_ordinal,
                    projection_refs: vec![projection_ref],
                    mutation: PreviewMutation::InvalidateRelationship {
                        relationship: effect.relationship.clone(),
                        source: source.clone(),
                    },
                });
                recoveries.push(relationship_recovery(
                    occurrence_ordinal,
                    projection_ref,
                    effect.relationship.clone(),
                    source,
                ));
            }
            (_, None, _) => recoveries.push(model_recovery(
                occurrence_ordinal,
                projection_ref,
                effect.source_model.clone(),
                partition.clone(),
            )),
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn lower_invalidations(
    occurrence_ordinal: u32,
    projection_ref: u32,
    event: &ManifestProjectionEventRef,
    operation: &ManifestProjectionOperation,
    partition: &Option<PreviewPartition>,
    slots: &BTreeMap<&str, Knowledge>,
    _manifest: &ClientManifest,
    operations: &mut Vec<PreviewOperation>,
    recoveries: &mut Vec<PreviewRecovery>,
) -> Result<(), ClientCompileError> {
    let partition = partition.clone();
    if operation.kind == ManifestProjectionMutationKind::InvalidateModel {
        operations.push(PreviewOperation {
            occurrence_ordinal,
            projection_refs: vec![projection_ref],
            mutation: PreviewMutation::InvalidateModel {
                partition: partition.clone(),
                model: operation.model.clone(),
            },
        });
        recoveries.push(model_recovery(
            occurrence_ordinal,
            projection_ref,
            operation.model.clone(),
            partition.clone(),
        ));
    }
    for invalidation in &operation.invalidations {
        match invalidation {
            ManifestProjectionInvalidation::Model { model } => {
                operations.push(PreviewOperation {
                    occurrence_ordinal,
                    projection_refs: vec![projection_ref],
                    mutation: PreviewMutation::InvalidateModel {
                        partition: partition.clone(),
                        model: model.clone(),
                    },
                });
                recoveries.push(model_recovery(
                    occurrence_ordinal,
                    projection_ref,
                    model.clone(),
                    partition.clone(),
                ));
            }
            ManifestProjectionInvalidation::Relationship {
                source_model,
                relationship,
                target_model: _,
            } => {
                let source = partition
                    .clone()
                    .zip(evaluate_key(&operation.key, slots, event))
                    .map(|(partition, key)| PreviewScope {
                        partition,
                        model: source_model.clone(),
                        key,
                    });
                if let Some(source) = source {
                    operations.push(PreviewOperation {
                        occurrence_ordinal,
                        projection_refs: vec![projection_ref],
                        mutation: PreviewMutation::InvalidateRelationship {
                            relationship: relationship.clone(),
                            source: source.clone(),
                        },
                    });
                    recoveries.push(relationship_recovery(
                        occurrence_ordinal,
                        projection_ref,
                        relationship.clone(),
                        source,
                    ));
                } else {
                    recoveries.push(model_recovery(
                        occurrence_ordinal,
                        projection_ref,
                        source_model.clone(),
                        partition.clone(),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn evaluate_partition(
    partition: &ManifestProjectionPartition,
    slots: &BTreeMap<&str, Knowledge>,
    event: &ManifestProjectionEventRef,
) -> Option<PreviewPartition> {
    match partition {
        ManifestProjectionPartition::Unit => Some(PreviewPartition::Unit),
        ManifestProjectionPartition::Expression { expression } => {
            match evaluate_expression(expression, slots, event) {
                // This is an executable logical expression, never a server
                // projection-partition token. The runtime must prove it maps
                // to the replica's already-authorized current cache partition
                // before applying any row or edge mutation.
                Knowledge::Known(expression) => Some(PreviewPartition::Expression {
                    expression,
                    requires: PreviewPartitionRequirement::CurrentCachePartition,
                }),
                Knowledge::Unknown
                | Knowledge::Unset
                | Knowledge::Denied
                | Knowledge::CacheUnowned => None,
            }
        }
    }
}

fn evaluate_key(
    key: &[ManifestProjectionKeyField],
    slots: &BTreeMap<&str, Knowledge>,
    event: &ManifestProjectionEventRef,
) -> Option<Vec<PreviewKeyField>> {
    let mut result = Vec::with_capacity(key.len());
    for field in key {
        let Knowledge::Known(value) = evaluate_expression(&field.expression, slots, event) else {
            return None;
        };
        result.push(PreviewKeyField {
            ordinal: field.ordinal,
            field: field.name.clone(),
            value,
        });
    }
    Some(result)
}

fn evaluate_fields(
    fields: &[ManifestProjectionField],
    slots: &BTreeMap<&str, Knowledge>,
    event: &ManifestProjectionEventRef,
) -> (Vec<PreviewField>, Vec<String>, bool) {
    let mut set = Vec::new();
    let mut unset = Vec::new();
    let mut uncertain = false;
    for field in fields {
        match &field.assignment {
            ManifestProjectionAssignment::Unset => unset.push(field.name.clone()),
            ManifestProjectionAssignment::Set { expression } => {
                match evaluate_expression(expression, slots, event) {
                    Knowledge::Known(value) => set.push(PreviewField {
                        field: field.name.clone(),
                        value,
                    }),
                    Knowledge::Unset => unset.push(field.name.clone()),
                    Knowledge::Unknown | Knowledge::Denied | Knowledge::CacheUnowned => {
                        uncertain = true;
                    }
                }
            }
        }
    }
    set.sort_by(|left, right| left.field.cmp(&right.field));
    unset.sort();
    (set, unset, uncertain)
}

fn source_knowledge(source: &ManifestProjectionPreviewSource) -> Knowledge {
    match source {
        ManifestProjectionPreviewSource::Input { path } => {
            Knowledge::Known(PreviewExpression::Input { path: path.clone() })
        }
        ManifestProjectionPreviewSource::GeneratedDefault { path } => {
            Knowledge::Known(PreviewExpression::GeneratedDefault { path: path.clone() })
        }
        ManifestProjectionPreviewSource::TrustedPreset { name, codec } => {
            Knowledge::Known(PreviewExpression::TrustedPreset {
                name: name.clone(),
                codec: codec.clone(),
            })
        }
        ManifestProjectionPreviewSource::Constant { value } => {
            Knowledge::Known(PreviewExpression::Constant {
                value: value.clone(),
            })
        }
        ManifestProjectionPreviewSource::Null => Knowledge::Known(PreviewExpression::Null),
        ManifestProjectionPreviewSource::Absent => Knowledge::Unset,
        ManifestProjectionPreviewSource::Unknown => Knowledge::Unknown,
    }
}

fn evaluate_expression(
    expression: &ManifestProjectionExpression,
    slots: &BTreeMap<&str, Knowledge>,
    event: &ManifestProjectionEventRef,
) -> Knowledge {
    match expression {
        ManifestProjectionExpression::Slot { slot, .. } => slots
            .get(slot.as_str())
            .cloned()
            .unwrap_or(Knowledge::Denied),
        ManifestProjectionExpression::Envelope { field } => {
            let value = match field {
                ManifestProjectionEnvelopeField::OccurrenceVersion => {
                    ManifestProjectionValue::U64("1".into())
                }
                ManifestProjectionEnvelopeField::EventName => {
                    ManifestProjectionValue::String(event.name.clone())
                }
                ManifestProjectionEnvelopeField::EventVersion => {
                    ManifestProjectionValue::U64(event.version.to_string())
                }
            };
            Knowledge::Known(PreviewExpression::Constant { value })
        }
        ManifestProjectionExpression::Constant { value } => {
            Knowledge::Known(PreviewExpression::Constant {
                value: value.clone(),
            })
        }
        ManifestProjectionExpression::Enum { enum_type, variant } => {
            Knowledge::Known(PreviewExpression::Constant {
                value: ManifestProjectionValue::Enum {
                    enum_type: enum_type.clone(),
                    variant: variant.clone(),
                },
            })
        }
        ManifestProjectionExpression::List { values } => {
            let mut resolved = Vec::with_capacity(values.len());
            for value in values {
                match evaluate_expression(value, slots, event) {
                    Knowledge::Known(value) => resolved.push(value),
                    other => return other,
                }
            }
            Knowledge::Known(PreviewExpression::List { values: resolved })
        }
        ManifestProjectionExpression::Object { fields } => {
            let mut resolved = Vec::new();
            for field in fields {
                match evaluate_expression(&field.value, slots, event) {
                    Knowledge::Known(value) => resolved.push(PreviewObjectField {
                        name: field.name.clone(),
                        value,
                    }),
                    Knowledge::Unset => {}
                    other => return other,
                }
            }
            Knowledge::Known(PreviewExpression::Object { fields: resolved })
        }
        ManifestProjectionExpression::Transform {
            transform,
            arguments,
        } => {
            let mut resolved = Vec::new();
            for argument in arguments {
                match evaluate_expression(argument, slots, event) {
                    Knowledge::Known(value) => resolved.push(value),
                    Knowledge::Unset
                        if *transform == ManifestProjectionScalarTransform::FirstPresent => {}
                    other => return other,
                }
            }
            if resolved.is_empty() {
                Knowledge::Unset
            } else {
                Knowledge::Known(PreviewExpression::Transform {
                    transform: *transform,
                    arguments: resolved,
                })
            }
        }
    }
}

fn canonicalize_operations(
    operations: Vec<PreviewOperation>,
) -> Result<Vec<PreviewOperation>, ClientCompileError> {
    let mut by_scope = BTreeMap::<PreviewOperationScope, PreviewOperation>::new();
    for operation in operations {
        let scope = operation.mutation.canonical_scope()?;
        if let Some(existing) = by_scope.get_mut(&scope) {
            merge_operation(existing, operation)?;
        } else {
            by_scope.insert(scope, operation);
        }
    }
    Ok(by_scope.into_values().collect())
}

fn canonicalize_recoveries(
    recoveries: Vec<PreviewRecovery>,
    operations: &[PreviewOperation],
) -> Result<Vec<PreviewRecovery>, ClientCompileError> {
    let mut by_target = BTreeMap::<PreviewRecoveryTargetKey, PreviewRecovery>::new();
    for recovery in recoveries {
        let target = recovery.target.canonical_key()?;
        by_target
            .entry(target)
            .and_modify(|existing| {
                existing.occurrence_ordinal =
                    existing.occurrence_ordinal.max(recovery.occurrence_ordinal);
                existing.projection_refs.extend(&recovery.projection_refs);
                existing.projection_refs.sort();
                existing.projection_refs.dedup();
                if recovery.condition == PreviewRecoveryCondition::Always {
                    existing.condition = PreviewRecoveryCondition::Always;
                }
            })
            .or_insert(recovery);
    }
    Ok(by_target
        .into_values()
        .filter(|recovery| {
            if recovery.condition == PreviewRecoveryCondition::Always {
                return true;
            }
            let PreviewRecoveryTarget::Record { scope } = &recovery.target else {
                return false;
            };
            operations.iter().any(|operation| {
                matches!(
                    &operation.mutation,
                    PreviewMutation::Patch {
                        scope: patch_scope,
                        if_present: true,
                        ..
                    } if patch_scope == scope
                )
            })
        })
        .collect())
}

fn merge_operation(
    existing: &mut PreviewOperation,
    incoming: PreviewOperation,
) -> Result<(), ClientCompileError> {
    if existing.occurrence_ordinal == incoming.occurrence_ordinal {
        if existing.mutation != incoming.mutation {
            return Err(incompatible_final_mutations(
                "same occurrence cannot contribute different mutations to one scope",
            ));
        }
        merge_refs(&mut existing.projection_refs, incoming.projection_refs);
        return Ok(());
    }
    let (earlier, later) = if existing.occurrence_ordinal < incoming.occurrence_ordinal {
        (existing.mutation.clone(), incoming.mutation)
    } else {
        (incoming.mutation, existing.mutation.clone())
    };
    existing.mutation = merge_mutation(earlier, later)?;
    existing.occurrence_ordinal = existing.occurrence_ordinal.max(incoming.occurrence_ordinal);
    merge_refs(&mut existing.projection_refs, incoming.projection_refs);
    Ok(())
}

fn merge_mutation(
    existing: PreviewMutation,
    incoming: PreviewMutation,
) -> Result<PreviewMutation, ClientCompileError> {
    if existing.canonical_scope()? != incoming.canonical_scope()? {
        return Err(incompatible_final_mutations(
            "same scope resolves to incompatible final mutations",
        ));
    }
    match (existing, incoming) {
        (
            PreviewMutation::Patch {
                scope,
                set,
                unset,
                if_present,
            },
            PreviewMutation::Patch {
                set: incoming_set,
                unset: incoming_unset,
                if_present: incoming_present,
                ..
            },
        ) => {
            let mut values = set
                .into_iter()
                .map(|field| (field.field.clone(), Some(field)))
                .collect::<BTreeMap<_, _>>();
            for field in unset {
                values.insert(field, None);
            }
            for field in incoming_set {
                values.insert(field.field.clone(), Some(field));
            }
            for field in incoming_unset {
                values.insert(field, None);
            }
            let set = values.values().filter_map(Clone::clone).collect();
            let unset = values
                .into_iter()
                .filter_map(|(field, value)| value.is_none().then_some(field))
                .collect();
            Ok(PreviewMutation::Patch {
                scope,
                set,
                unset,
                if_present: if_present && incoming_present,
            })
        }
        (
            PreviewMutation::Upsert {
                scope,
                fields,
                replace,
            },
            PreviewMutation::Patch {
                set,
                unset,
                if_present: true,
                ..
            },
        ) => {
            let mut values = fields
                .into_iter()
                .map(|field| (field.field.clone(), field))
                .collect::<BTreeMap<_, _>>();
            let replace = replace.into_iter().collect::<BTreeSet<_>>();
            if set
                .iter()
                .map(|field| &field.field)
                .chain(unset.iter())
                .any(|field| !replace.contains(field))
            {
                return Err(incompatible_final_mutations(
                    "patch fields must belong to the upsert replacement mask",
                ));
            }
            for field in set {
                values.insert(field.field.clone(), field);
            }
            for field in unset {
                values.remove(&field);
            }
            Ok(PreviewMutation::Upsert {
                scope,
                fields: values.into_values().collect(),
                replace: replace.into_iter().collect(),
            })
        }
        (PreviewMutation::Patch { .. }, incoming @ PreviewMutation::Upsert { .. })
        | (PreviewMutation::Patch { .. }, incoming @ PreviewMutation::Delete { .. })
        | (PreviewMutation::Upsert { .. }, incoming @ PreviewMutation::Upsert { .. })
        | (PreviewMutation::Upsert { .. }, incoming @ PreviewMutation::Delete { .. })
        | (PreviewMutation::Delete { .. }, incoming @ PreviewMutation::Upsert { .. })
        | (PreviewMutation::Delete { .. }, incoming @ PreviewMutation::Delete { .. }) => {
            Ok(incoming)
        }
        (existing @ PreviewMutation::Delete { .. }, PreviewMutation::Patch { .. }) => Ok(existing),
        (
            PreviewMutation::Link { .. } | PreviewMutation::Unlink { .. },
            incoming @ (PreviewMutation::Link { .. } | PreviewMutation::Unlink { .. }),
        )
        | (
            PreviewMutation::InvalidateModel { .. },
            incoming @ PreviewMutation::InvalidateModel { .. },
        )
        | (
            PreviewMutation::InvalidateRelationship { .. },
            incoming @ PreviewMutation::InvalidateRelationship { .. },
        ) => Ok(incoming),
        _ => Err(incompatible_final_mutations(
            "same scope resolves to incompatible final mutations",
        )),
    }
}

fn merge_refs(existing: &mut Vec<u32>, incoming: Vec<u32>) {
    let mut refs = existing.iter().copied().collect::<BTreeSet<_>>();
    refs.extend(incoming);
    *existing = refs.into_iter().collect();
}

fn incompatible_final_mutations(message: &'static str) -> ClientCompileError {
    ClientCompileError::manifest("client.projection_preview.ambiguous_scope", message)
}

fn record_recovery(
    occurrence_ordinal: u32,
    projection_ref: u32,
    scope: PreviewScope,
    condition: PreviewRecoveryCondition,
) -> PreviewRecovery {
    PreviewRecovery {
        occurrence_ordinal,
        projection_refs: vec![projection_ref],
        condition,
        target: PreviewRecoveryTarget::Record { scope },
    }
}

fn relationship_recovery(
    occurrence_ordinal: u32,
    projection_ref: u32,
    relationship: String,
    source: PreviewScope,
) -> PreviewRecovery {
    PreviewRecovery {
        occurrence_ordinal,
        projection_refs: vec![projection_ref],
        condition: PreviewRecoveryCondition::Always,
        target: PreviewRecoveryTarget::Relationship {
            relationship,
            source,
        },
    }
}

fn model_recovery(
    occurrence_ordinal: u32,
    projection_ref: u32,
    model: String,
    partition: Option<PreviewPartition>,
) -> PreviewRecovery {
    PreviewRecovery {
        occurrence_ordinal,
        projection_refs: vec![projection_ref],
        condition: PreviewRecoveryCondition::Always,
        target: PreviewRecoveryTarget::Model { partition, model },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event() -> ManifestProjectionEventRef {
        ManifestProjectionEventRef {
            id: "event:test:v1".into(),
            name: "test.changed".into(),
            version: 1,
        }
    }

    fn slot() -> ManifestProjectionExpression {
        ManifestProjectionExpression::Slot {
            slot: "value".into(),
            value_type: ManifestProjectionValueType::String,
        }
    }

    fn preview_scope(model: &str) -> PreviewScope {
        PreviewScope {
            partition: PreviewPartition::Unit,
            model: model.into(),
            key: vec![PreviewKeyField {
                ordinal: 0,
                field: "id".into(),
                value: PreviewExpression::Input {
                    path: vec!["id".into()],
                },
            }],
        }
    }

    fn preview_field(name: &str, path: &str) -> PreviewField {
        PreviewField {
            field: name.into(),
            value: PreviewExpression::Input {
                path: vec![path.into()],
            },
        }
    }

    fn operation(occurrence_ordinal: u32, mutation: PreviewMutation) -> PreviewOperation {
        PreviewOperation {
            occurrence_ordinal,
            projection_refs: vec![0],
            mutation,
        }
    }

    #[test]
    fn knowledge_lattice_preserves_null_unset_unknown_denied_and_cache_unowned() {
        let event = event();
        let cases = [
            Knowledge::Known(PreviewExpression::Null),
            Knowledge::Unset,
            Knowledge::Unknown,
            Knowledge::Denied,
            Knowledge::CacheUnowned,
        ];
        for expected in cases {
            let slots = BTreeMap::from([("value", expected.clone())]);
            assert_eq!(evaluate_expression(&slot(), &slots, &event), expected);
        }
    }

    #[test]
    fn denied_or_cache_unowned_nested_dependencies_never_become_values() {
        let expression = ManifestProjectionExpression::Object {
            fields: vec![ManifestProjectionObjectField {
                name: "secret".into(),
                value: slot(),
            }],
        };
        for expected in [Knowledge::Denied, Knowledge::CacheUnowned] {
            let slots = BTreeMap::from([("value", expected.clone())]);
            assert_eq!(evaluate_expression(&expression, &slots, &event()), expected);
        }
    }

    #[test]
    fn known_partition_expression_requires_current_cache_proof() {
        let expression = ManifestProjectionPartition::Expression { expression: slot() };
        let known = BTreeMap::from([(
            "value",
            Knowledge::Known(PreviewExpression::Input {
                path: vec!["tenant_id".into()],
            }),
        )]);
        let partition = evaluate_partition(&expression, &known, &event()).unwrap();
        let rendered = serde_json::to_value(partition).unwrap();
        assert_eq!(rendered["kind"], "expression");
        assert_eq!(rendered["requires"], "current_cache_partition");
        assert_eq!(
            rendered["expression"],
            serde_json::json!({"kind": "input", "path": ["tenant_id"]})
        );
        assert!(rendered.get("token").is_none());
    }

    #[test]
    fn uncertain_or_unowned_partition_never_addresses_a_row() {
        let expression = ManifestProjectionPartition::Expression { expression: slot() };
        for knowledge in [
            Knowledge::Unknown,
            Knowledge::Unset,
            Knowledge::Denied,
            Knowledge::CacheUnowned,
        ] {
            let slots = BTreeMap::from([("value", knowledge)]);
            assert!(evaluate_partition(&expression, &slots, &event()).is_none());
        }
    }

    #[test]
    fn canonical_merge_is_occurrence_ordered_and_input_permutation_independent() {
        let scope = preview_scope("Todo");
        let upsert = operation(
            0,
            PreviewMutation::Upsert {
                scope: scope.clone(),
                fields: vec![
                    preview_field("status", "created_status"),
                    preview_field("title", "title"),
                ],
                replace: vec!["status".into(), "title".into()],
            },
        );
        let patch = operation(
            1,
            PreviewMutation::Patch {
                scope: scope.clone(),
                set: vec![preview_field("status", "completed_status")],
                unset: Vec::new(),
                if_present: true,
            },
        );

        let forward = canonicalize_operations(vec![upsert.clone(), patch.clone()]).unwrap();
        let reversed = canonicalize_operations(vec![patch, upsert]).unwrap();
        assert_eq!(forward, reversed);
        assert_eq!(forward[0].occurrence_ordinal, 1);
        let PreviewMutation::Upsert {
            fields, replace, ..
        } = &forward[0].mutation
        else {
            panic!("upsert followed by a patch remains an upsert");
        };
        assert_eq!(replace, &vec!["status".to_string(), "title".to_string()]);
        assert_eq!(
            fields
                .iter()
                .find(|field| field.field == "status")
                .unwrap()
                .value,
            PreviewExpression::Input {
                path: vec!["completed_status".into()]
            }
        );
    }

    #[test]
    fn canonical_merge_preserves_delete_precedence_and_structured_kind_order() {
        let record = preview_scope("Todo");
        let target = preview_scope("User");
        let upsert = operation(
            0,
            PreviewMutation::Upsert {
                scope: record.clone(),
                fields: vec![preview_field("title", "title")],
                replace: vec!["title".into()],
            },
        );
        let delete = operation(
            2,
            PreviewMutation::Delete {
                scope: record.clone(),
            },
        );
        let link = operation(
            0,
            PreviewMutation::Link {
                relationship: "owner".into(),
                source: record.clone(),
                target,
            },
        );
        let model = operation(
            0,
            PreviewMutation::InvalidateModel {
                partition: Some(PreviewPartition::Unit),
                model: "Todo".into(),
            },
        );
        let relationship = operation(
            0,
            PreviewMutation::InvalidateRelationship {
                relationship: "owner".into(),
                source: record,
            },
        );
        let canonical =
            canonicalize_operations(vec![relationship, model, link, delete, upsert]).unwrap();
        assert!(matches!(
            canonical[0].mutation,
            PreviewMutation::Delete { .. }
        ));
        assert!(matches!(
            canonical[1].mutation,
            PreviewMutation::Link { .. }
        ));
        assert!(matches!(
            canonical[2].mutation,
            PreviewMutation::InvalidateModel { .. }
        ));
        assert!(matches!(
            canonical[3].mutation,
            PreviewMutation::InvalidateRelationship { .. }
        ));
    }

    #[test]
    fn conditional_recovery_is_removed_when_final_operation_is_not_a_patch() {
        let scope = preview_scope("Todo");
        let operations = vec![operation(
            1,
            PreviewMutation::Delete {
                scope: scope.clone(),
            },
        )];
        let recovery = record_recovery(0, 0, scope, PreviewRecoveryCondition::IfRecordMissing);
        assert!(canonicalize_recoveries(vec![recovery], &operations)
            .unwrap()
            .is_empty());
    }
}
