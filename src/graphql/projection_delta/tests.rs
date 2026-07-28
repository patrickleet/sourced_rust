use std::borrow::Cow;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::time::{Duration, UNIX_EPOCH};

use serde::Serialize;
use serde_json::json;

use super::authorization::{
    ProjectionPartitionAuthority, ProjectionPartitionScopeEncoder, ProjectionVisibilityEvaluator,
};
use super::lower::{
    ProjectionDeltaAuthority, ProjectionDeltaPlanOccurrence, ProjectionDeltaRequestAuthority,
};
use super::types::{
    AuthorizationTransition, ProjectionDeltaCacheScopeToken, ProjectionDeltaVisibility,
    ProjectionMutationSource,
};
use super::*;
use crate::graphql::{
    build_surface, surface_for_role, DistributedClientSurfaceExport, RoleGrant, SurfaceOptions,
    SurfaceProjector,
};
use crate::projection::catalog::ProjectionBindingActivation;
use crate::projection::placement::{
    ProjectionBinding, ProjectionBindingState, ProjectionExecutionClass, ProjectionExecutorRoute,
    ProjectionOutput, ProjectionOwner, ProjectionPhysicalTopology, ProjectionRelationshipBinding,
    ProjectionSourceBinding, PROJECTION_PARTITION_CODEC_VERSION,
};
use crate::projection::{
    ProjectionArm, ProjectionField, ProjectionOperation, ProjectionPartition, ProjectionTarget,
};
use crate::projection_protocol::{ProjectionEpoch, ProjectorTopologyId};
use crate::table::{
    ColumnType, PrimaryKey, RelationshipDef, RelationshipKind, TableColumn, TableKind, TableSchema,
};
use crate::{
    DomainEventBodyDescriptor, DomainEventBodyKind, DomainEventDescriptor, DomainEventEnvelope,
    DomainEventOccurrence, ProjectionAssignment, ProjectionExpression, ProjectionInvalidation,
    ProjectionKeyField, ProjectionMutationKind, ProjectionProgram, ProjectionRelationship,
    ProjectionRelationshipEffect, ProjectionValue, ProjectionValueType, ResolvedProjectionMutation,
    ResolvedProjectionRelationshipEffect, DOMAIN_EVENT_BODY_CODEC, DOMAIN_EVENT_BODY_CODEC_VERSION,
    MAX_PROJECTION_EXPRESSION_DEPTH,
};

const BODY_FINGERPRINT: &str =
    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[test]
fn strict_decode_rejects_unknown_wire_fields() {
    let bytes = br#"{"wire_version":1,"identity":{},"projections":[],"occurrences":[],"operations":[],"recoveries":[],"unknown":true}"#;
    assert!(matches!(
        ProjectionDelta::from_json(bytes),
        Err(ProjectionDeltaError::InvalidWire(_))
    ));
}

#[test]
fn authoritative_zero_actual_occurrences_is_valid_and_deterministic() {
    let delta = empty_delta();
    let bytes = delta.canonical_bytes().unwrap();
    assert_eq!(ProjectionDelta::from_json(&bytes).unwrap(), delta);
    assert!(delta.projections.is_empty());
    assert!(delta.occurrences.is_empty());
    assert!(delta.operations.is_empty());
}

#[test]
fn upsert_requires_exact_non_key_mask_and_allows_key_only_rows() {
    let scope = test_scope("tenant-a", "KeyOnly", vec![key_field(0, "id", "1")]);
    let key_only = operation(
        0,
        ProjectionDeltaMutation::Upsert {
            scope: scope.clone(),
            fields: vec![],
            replace: vec![],
        },
    );
    key_only.validate(1, 1).unwrap();

    let mismatched = operation(
        0,
        ProjectionDeltaMutation::Upsert {
            scope,
            fields: vec![DeltaField {
                field: "secret".into(),
                value: DeltaValue::String("safe".into()),
            }],
            replace: vec!["status".into(), "title".into()],
        },
    );
    assert!(matches!(
        mismatched.validate(1, 1),
        Err(ProjectionDeltaError::InvalidOperation(
            "upsert fields must be contained in the replacement mask"
        ))
    ));
}

#[test]
fn composite_keys_preserve_declared_order_instead_of_name_order() {
    let scope = test_scope(
        "tenant-a",
        "Memberships",
        vec![
            key_field(0, "z_tenant_id", "tenant-a"),
            key_field(1, "a_user_id", "user-1"),
        ],
    );
    scope.validate().unwrap();
    assert_eq!(scope.key[0].field, "z_tenant_id");
    assert_eq!(scope.key[1].field, "a_user_id");
}

#[test]
fn tagged_values_distinguish_null_absent_and_unset_and_bound_numbers() {
    DeltaValue::Null.validate().unwrap();
    DeltaValue::I64(i64::MIN.to_string()).validate().unwrap();
    DeltaValue::U64(u64::MAX.to_string()).validate().unwrap();
    DeltaValue::F64("1.5".into()).validate().unwrap();
    assert_eq!(
        serde_json::to_value(DeltaValue::Null).unwrap(),
        json!({"type": "null"})
    );
    assert!(matches!(
        DeltaValue::I64("01".into()).validate(),
        Err(ProjectionDeltaError::InvalidNumber)
    ));
    assert!(matches!(
        DeltaValue::F64("-0".into()).validate(),
        Err(ProjectionDeltaError::InvalidNumber)
    ));

    let patch = ProjectionDeltaMutation::Patch {
        scope: test_scope("tenant-a", "Todos", vec![key_field(0, "todo_id", "todo-1")]),
        set: vec![DeltaField {
            field: "nullable_note".into(),
            value: DeltaValue::Null,
        }],
        unset: vec!["title".into()],
        if_present: true,
    };
    let json = serde_json::to_value(patch).unwrap();
    assert!(json["set"][0].get("value").is_some());
    assert_eq!(json["unset"], json!(["title"]));
    assert!(json.get("absent").is_none());
}

#[test]
fn value_depth_key_partition_and_body_limits_fail_closed() {
    let mut value = DeltaValue::String("leaf".into());
    for _ in 0..MAX_PROJECTION_EXPRESSION_DEPTH {
        value = DeltaValue::List(vec![value]);
    }
    assert!(matches!(
        value.validate(),
        Err(ProjectionDeltaError::ValueTooDeep { .. })
    ));
    let oversized_partition = ProjectionDeltaPartition::Opaque {
        token: "x".repeat(crate::projection_protocol::MAX_PROJECTION_PARTITION_BYTES),
    };
    assert!(matches!(
        oversized_partition.validate(),
        Err(ProjectionDeltaError::PartitionTooLarge { .. })
    ));
    let oversized_body = vec![b' '; crate::MAX_DOMAIN_EVENT_BODY_BYTES + 1];
    assert!(matches!(
        ProjectionDelta::from_json(&oversized_body),
        Err(ProjectionDeltaError::BodyTooLarge { .. })
    ));
}

#[test]
fn sealed_authority_lowers_actual_full_state_and_memoizes_partition() {
    let fixture = modeled_fixture(
        ProjectionBindingState::Active,
        ProjectionExecutionClass::Causal,
    );
    let selected = selected_export(&fixture.surface);
    let request = TestRequestAuthority::authorized("auth-generation-7");
    let authority = ProjectionDeltaAuthority::try_new(&selected, &request).unwrap();
    let modeled = selected.surface().projectors[0].modeled[0].clone();
    let source = authority.source(&modeled).unwrap();
    let first = crate::ResolvedProjectionPlan::resolve(
        &fixture.program,
        &state_occurrence(7, "todo-1", "Résumé 🚀"),
    )
    .unwrap();
    let second = crate::ResolvedProjectionPlan::resolve(
        &fixture.program,
        &state_occurrence(8, "todo-1", "Résumé 🚀"),
    )
    .unwrap();
    let batch = vec![
        ProjectionDeltaPlanOccurrence::actual(vec![(&source, &first)]).unwrap(),
        ProjectionDeltaPlanOccurrence::actual(vec![(&source, &second)]).unwrap(),
    ];
    let delta = authority.lower(&batch).unwrap();

    assert_eq!(request.calls.get(), 1);
    assert_eq!(delta.occurrences.len(), 2);
    let upsert = delta
        .operations
        .iter()
        .find_map(|operation| match &operation.mutation {
            ProjectionDeltaMutation::Upsert {
                scope,
                fields,
                replace,
            } => Some((scope, fields, replace)),
            _ => None,
        })
        .expect("full actual state lowers to upsert");
    assert_eq!(upsert.0.model, "TodoView");
    assert_eq!(
        upsert
            .1
            .iter()
            .map(|field| field.field.as_str())
            .collect::<Vec<_>>(),
        vec!["owner_id", "status", "title"]
    );
    assert_eq!(upsert.2, &vec!["owner_id", "status", "title"]);
    assert!(upsert.1.iter().all(|field| field.field != "todo_id"));
    assert!(delta
        .operations
        .iter()
        .any(|operation| matches!(operation.mutation, ProjectionDeltaMutation::Link { .. })));
}

#[test]
fn full_state_preview_lowers_to_upsert() {
    let fixture = modeled_fixture(
        ProjectionBindingState::Active,
        ProjectionExecutionClass::Causal,
    );
    let selected = selected_export(&fixture.surface);
    let request = TestRequestAuthority::authorized("auth-generation-7");
    let authority = ProjectionDeltaAuthority::try_new(&selected, &request).unwrap();
    let modeled = selected.surface().projectors[0].modeled[0].clone();
    let source = authority.source(&modeled).unwrap();
    let plan = crate::ResolvedProjectionPlan::resolve(
        &fixture.program,
        &state_occurrence(7, "todo-1", "preview"),
    )
    .unwrap();
    let occurrence = ProjectionDeltaPlanOccurrence::preview(vec![(&source, &plan)]).unwrap();
    let delta = authority.lower(&[occurrence]).unwrap();
    assert!(delta
        .operations
        .iter()
        .any(|operation| matches!(operation.mutation, ProjectionDeltaMutation::Upsert { .. })));
    assert!(delta.recoveries.is_empty());
}

#[test]
fn partial_preview_lowers_to_conditional_patch_with_missing_record_fallback() {
    let fixture = modeled_fixture_with_kind(
        ProjectionBindingState::Active,
        ProjectionExecutionClass::Causal,
        ProjectionMutationKind::Patch,
    );
    let selected = selected_export(&fixture.surface);
    let request = TestRequestAuthority::authorized("auth-generation-7");
    let authority = ProjectionDeltaAuthority::try_new(&selected, &request).unwrap();
    let modeled = selected.surface().projectors[0].modeled[0].clone();
    let source = authority.source(&modeled).unwrap();
    let plan = crate::ResolvedProjectionPlan::resolve(
        &fixture.program,
        &partial_state_occurrence(7, "todo-1"),
    )
    .unwrap();
    let occurrence = ProjectionDeltaPlanOccurrence::preview(vec![(&source, &plan)]).unwrap();
    let delta = authority.lower(&[occurrence]).unwrap();
    assert!(delta.operations.iter().any(|operation| matches!(
        operation.mutation,
        ProjectionDeltaMutation::Patch {
            if_present: true,
            ..
        }
    )));
    assert!(delta.recoveries.iter().any(|recovery| {
        recovery.condition == ProjectionDeltaRecoveryCondition::IfRecordMissing
            && matches!(
                recovery.target,
                ProjectionDeltaRecoveryTarget::Record { .. }
            )
    }));
}

#[test]
fn value_partition_encoder_cannot_collapse_scope_to_unit() {
    let fixture = modeled_fixture(
        ProjectionBindingState::Active,
        ProjectionExecutionClass::Causal,
    );
    let selected = selected_export(&fixture.surface);
    let mut request = TestRequestAuthority::authorized("auth-generation-7");
    request.partition_result = PartitionResult::Unit;
    let authority = ProjectionDeltaAuthority::try_new(&selected, &request).unwrap();
    let modeled = selected.surface().projectors[0].modeled[0].clone();
    let source = authority.source(&modeled).unwrap();
    let plan = crate::ResolvedProjectionPlan::resolve(
        &fixture.program,
        &state_occurrence(7, "todo-1", "title"),
    )
    .unwrap();
    let occurrence = ProjectionDeltaPlanOccurrence::actual(vec![(&source, &plan)]).unwrap();
    assert_eq!(
        authority.lower(&[occurrence]),
        Err(ProjectionDeltaError::AuthorizationMapping)
    );
}

#[test]
fn unknown_visibility_emits_only_model_recovery_without_record_or_endpoint_keys() {
    let fixture = modeled_fixture(
        ProjectionBindingState::Active,
        ProjectionExecutionClass::Causal,
    );
    let selected = selected_export(&fixture.surface);
    let unknown = AuthorizationTransition {
        before: ProjectionDeltaVisibility::Unknown,
        after: ProjectionDeltaVisibility::Unknown,
    };
    let request = TestRequestAuthority::new("auth-generation-7", unknown, unknown);
    let authority = ProjectionDeltaAuthority::try_new(&selected, &request).unwrap();
    let modeled = selected.surface().projectors[0].modeled[0].clone();
    let source = authority.source(&modeled).unwrap();
    let plan = crate::ResolvedProjectionPlan::resolve(
        &fixture.program,
        &state_occurrence(7, "todo-secret", "secret-title"),
    )
    .unwrap();
    let occurrence = ProjectionDeltaPlanOccurrence::actual(vec![(&source, &plan)]).unwrap();
    let delta = authority.lower(&[occurrence]).unwrap();
    assert!(delta.operations.iter().all(|operation| matches!(
        operation.mutation,
        ProjectionDeltaMutation::InvalidateModel { .. }
    )));
    assert!(delta.recoveries.iter().all(|recovery| {
        recovery.condition == ProjectionDeltaRecoveryCondition::Always
            && matches!(recovery.target, ProjectionDeltaRecoveryTarget::Model { .. })
    }));
    let json = String::from_utf8(delta.canonical_bytes().unwrap()).unwrap();
    assert!(!json.contains("todo-secret"));
    assert!(!json.contains("owner-secret"));
    assert!(!json.contains("secret-title"));
}

#[test]
fn hidden_relationship_target_transforms_link_into_source_invalidation() {
    let fixture = modeled_fixture(
        ProjectionBindingState::Active,
        ProjectionExecutionClass::Causal,
    );
    let selected = selected_export_with_embedded_target(&fixture.surface);
    let request = TestRequestAuthority::authorized("auth-generation-7");
    let authority = ProjectionDeltaAuthority::try_new(&selected, &request).unwrap();
    let modeled = selected.surface().projectors[0].modeled[0].clone();
    let source = authority.source(&modeled).unwrap();
    let plan = crate::ResolvedProjectionPlan::resolve(
        &fixture.program,
        &state_occurrence(7, "todo-1", "title"),
    )
    .unwrap();
    let occurrence = ProjectionDeltaPlanOccurrence::actual(vec![(&source, &plan)]).unwrap();
    let delta = authority.lower(&[occurrence]).unwrap();
    assert!(!delta.operations.iter().any(|operation| matches!(
        operation.mutation,
        ProjectionDeltaMutation::Link { .. } | ProjectionDeltaMutation::Unlink { .. }
    )));
    assert!(delta.operations.iter().any(|operation| matches!(
        operation.mutation,
        ProjectionDeltaMutation::InvalidateRelationship { .. }
    )));
}

#[test]
fn explicit_relationship_invalidation_survives_removed_keyed_effect_as_model_recovery() {
    let fixture = modeled_fixture_with_relationship_invalidation(
        ProjectionBindingState::Active,
        ProjectionExecutionClass::Causal,
    );
    let selected = selected_export_with_embedded_source(&fixture.surface);
    let request = TestRequestAuthority::authorized("auth-generation-7");
    let authority = ProjectionDeltaAuthority::try_new(&selected, &request).unwrap();
    let modeled = selected.surface().projectors[0].modeled[0].clone();
    let source = authority.source(&modeled).unwrap();
    let plan = crate::ResolvedProjectionPlan::resolve(
        &fixture.program,
        &state_occurrence(7, "todo-1", "secret-title"),
    )
    .unwrap();
    let occurrence = ProjectionDeltaPlanOccurrence::actual(vec![(&source, &plan)]).unwrap();
    let delta = authority.lower(&[occurrence]).unwrap();

    assert!(!delta.operations.iter().any(|operation| matches!(
        operation.mutation,
        ProjectionDeltaMutation::Link { .. }
            | ProjectionDeltaMutation::Unlink { .. }
            | ProjectionDeltaMutation::InvalidateRelationship { .. }
    )));
    assert!(delta.operations.iter().any(|operation| matches!(
        &operation.mutation,
        ProjectionDeltaMutation::InvalidateModel { model, .. } if model == "TodoView"
    )));
    assert!(delta.recoveries.iter().any(|recovery| {
        recovery.condition == ProjectionDeltaRecoveryCondition::Always
            && matches!(
                &recovery.target,
                ProjectionDeltaRecoveryTarget::Model { model, .. } if model == "TodoView"
            )
    }));
    let json = String::from_utf8(delta.canonical_bytes().unwrap()).unwrap();
    assert!(!json.contains("todo-1"));
    assert!(!json.contains("owner-secret"));
    assert!(!json.contains("secret-title"));
}

#[test]
fn hidden_relationship_source_emits_no_keyed_consequence() {
    let fixture = modeled_fixture(
        ProjectionBindingState::Active,
        ProjectionExecutionClass::Causal,
    );
    let selected = selected_export_with_embedded_source(&fixture.surface);
    let request = TestRequestAuthority::authorized("auth-generation-7");
    let authority = ProjectionDeltaAuthority::try_new(&selected, &request).unwrap();
    let modeled = selected.surface().projectors[0].modeled[0].clone();
    let source = authority.source(&modeled).unwrap();
    let plan = crate::ResolvedProjectionPlan::resolve(
        &fixture.program,
        &state_occurrence(7, "todo-secret", "secret-title"),
    )
    .unwrap();
    let occurrence = ProjectionDeltaPlanOccurrence::actual(vec![(&source, &plan)]).unwrap();
    let delta = authority.lower(&[occurrence]).unwrap();
    assert!(delta.operations.iter().any(|operation| matches!(
        &operation.mutation,
        ProjectionDeltaMutation::InvalidateModel { model, .. } if model == "TodoView"
    )));
    assert!(delta.recoveries.iter().any(|recovery| {
        recovery.condition == ProjectionDeltaRecoveryCondition::Always
            && matches!(
                &recovery.target,
                ProjectionDeltaRecoveryTarget::Model { model, .. } if model == "TodoView"
            )
    }));
    let json = String::from_utf8(delta.canonical_bytes().unwrap()).unwrap();
    assert!(!json.contains("todo-secret"));
    assert!(!json.contains("owner-secret"));
    assert!(!json.contains("secret-title"));
}

#[test]
fn recovery_condition_is_order_independent_and_obsolete_patch_fallback_is_dropped() {
    let scope = test_scope("tenant-a", "Todos", vec![key_field(0, "todo_id", "todo-1")]);
    let patch = operation(
        0,
        ProjectionDeltaMutation::Patch {
            scope: scope.clone(),
            set: vec![DeltaField {
                field: "title".into(),
                value: DeltaValue::String("new".into()),
            }],
            unset: vec![],
            if_present: true,
        },
    );
    let conditional = ProjectionDeltaRecovery {
        occurrence_ordinal: 0,
        projection_refs: vec![0],
        condition: ProjectionDeltaRecoveryCondition::IfRecordMissing,
        target: ProjectionDeltaRecoveryTarget::Record {
            scope: scope.clone(),
        },
    };
    let always = ProjectionDeltaRecovery {
        occurrence_ordinal: 1,
        projection_refs: vec![1],
        condition: ProjectionDeltaRecoveryCondition::Always,
        target: ProjectionDeltaRecoveryTarget::Record {
            scope: scope.clone(),
        },
    };
    for recoveries in [
        vec![conditional.clone(), always.clone()],
        vec![always.clone(), conditional.clone()],
    ] {
        let merged =
            super::canonical::canonicalize_recoveries(recoveries, std::slice::from_ref(&patch));
        assert_eq!(merged.len(), 1);
        assert_eq!(
            merged[0].condition,
            ProjectionDeltaRecoveryCondition::Always
        );
        assert_eq!(merged[0].projection_refs, vec![0, 1]);
    }

    let upsert = operation(
        1,
        ProjectionDeltaMutation::Upsert {
            scope,
            fields: vec![DeltaField {
                field: "title".into(),
                value: DeltaValue::String("final".into()),
            }],
            replace: vec!["title".into()],
        },
    );
    assert!(super::canonical::canonicalize_recoveries(vec![conditional], &[upsert]).is_empty());
}

#[test]
fn relationship_crossing_authorization_never_serializes_target_values() {
    let fixture = modeled_fixture(
        ProjectionBindingState::Active,
        ProjectionExecutionClass::Causal,
    );
    let selected = selected_export(&fixture.surface);
    let request = TestRequestAuthority::new(
        "auth-generation-7",
        authorized_transition(),
        AuthorizationTransition {
            before: ProjectionDeltaVisibility::Denied,
            after: ProjectionDeltaVisibility::Authorized,
        },
    );
    let authority = ProjectionDeltaAuthority::try_new(&selected, &request).unwrap();
    let modeled = selected.surface().projectors[0].modeled[0].clone();
    let source = authority.source(&modeled).unwrap();
    let plan = crate::ResolvedProjectionPlan::resolve(
        &fixture.program,
        &state_occurrence(7, "todo-secret", "safe-title"),
    )
    .unwrap();
    let occurrence = ProjectionDeltaPlanOccurrence::actual(vec![(&source, &plan)]).unwrap();
    let delta = authority.lower(&[occurrence]).unwrap();
    assert!(!delta.operations.iter().any(|operation| matches!(
        operation.mutation,
        ProjectionDeltaMutation::Link { .. } | ProjectionDeltaMutation::Unlink { .. }
    )));
    assert!(delta.operations.iter().any(|operation| matches!(
        operation.mutation,
        ProjectionDeltaMutation::InvalidateRelationship { .. }
    )));
    let relationship_json = serde_json::to_string(
        &delta
            .operations
            .iter()
            .filter(|operation| {
                matches!(
                    operation.mutation,
                    ProjectionDeltaMutation::Link { .. }
                        | ProjectionDeltaMutation::Unlink { .. }
                        | ProjectionDeltaMutation::InvalidateRelationship { .. }
                )
            })
            .collect::<Vec<_>>(),
    )
    .unwrap();
    assert!(!relationship_json.contains("owner-secret"));
}

#[test]
fn draining_accepts_actual_but_rejects_preview_and_background_rejects_all() {
    let draining = modeled_fixture(
        ProjectionBindingState::Draining,
        ProjectionExecutionClass::Causal,
    );
    let selected = selected_export(&draining.surface);
    let request = TestRequestAuthority::authorized("auth-generation");
    let authority = ProjectionDeltaAuthority::try_new(&selected, &request).unwrap();
    let modeled = selected.surface().projectors[0].modeled[0].clone();
    let source = authority.source(&modeled).unwrap();
    let plan = crate::ResolvedProjectionPlan::resolve(
        &draining.program,
        &state_occurrence(7, "todo-1", "title"),
    )
    .unwrap();
    ProjectionDeltaPlanOccurrence::actual(vec![(&source, &plan)]).unwrap();
    assert!(matches!(
        ProjectionDeltaPlanOccurrence::preview(vec![(&source, &plan)]),
        Err(ProjectionDeltaError::IneligibleBinding)
    ));

    let background = modeled_fixture(
        ProjectionBindingState::Active,
        ProjectionExecutionClass::Background,
    );
    let selected = selected_export(&background.surface);
    let authority = ProjectionDeltaAuthority::try_new(&selected, &request).unwrap();
    let modeled = selected.surface().projectors[0].modeled[0].clone();
    assert!(matches!(
        authority.source(&modeled),
        Err(ProjectionDeltaError::IneligibleBinding)
    ));
}

#[test]
fn replay_scope_requires_exact_manifest_protocol_surface_generation_and_cause() {
    let fixture = modeled_fixture(
        ProjectionBindingState::Active,
        ProjectionExecutionClass::Causal,
    );
    let selected = selected_export(&fixture.surface);
    let manifest = selected.manifest().unwrap();
    let request = TestRequestAuthority::authorized("generation-1");
    let mut delta = empty_delta();
    delta.identity = ProjectionDeltaIdentity {
        manifest_version: manifest.manifest_version,
        client_protocol_version: manifest.protocol_version,
        surface: ProjectionDeltaSurfaceIdentity::from(&manifest.surface),
        schema_fingerprint: manifest.schema_fingerprint.clone(),
        protocol_fingerprint: manifest.protocol_fingerprint.clone(),
        authorization_generation: "generation-1".into(),
        cache_scope_token: request.cache_scope().as_str().into(),
        command_causation_id: request.command_causation_id().as_str().into(),
    };
    delta.validate_replay_scope(&manifest, &request).unwrap();
    let other_request = TestRequestAuthority::authorized("generation-2");
    assert_eq!(
        delta.validate_replay_scope(&manifest, &other_request),
        Err(ProjectionDeltaError::ReplayScopeMismatch)
    );
}

struct ModeledFixture {
    surface: crate::graphql::Surface,
    program: ProjectionProgram,
}

fn modeled_fixture(
    state: ProjectionBindingState,
    execution: ProjectionExecutionClass,
) -> ModeledFixture {
    modeled_fixture_config(state, execution, ProjectionMutationKind::Upsert, false)
}

fn modeled_fixture_with_kind(
    state: ProjectionBindingState,
    execution: ProjectionExecutionClass,
    mutation_kind: ProjectionMutationKind,
) -> ModeledFixture {
    modeled_fixture_config(state, execution, mutation_kind, false)
}

fn modeled_fixture_with_relationship_invalidation(
    state: ProjectionBindingState,
    execution: ProjectionExecutionClass,
) -> ModeledFixture {
    modeled_fixture_config(state, execution, ProjectionMutationKind::Upsert, true)
}

fn modeled_fixture_config(
    state: ProjectionBindingState,
    execution: ProjectionExecutionClass,
    mutation_kind: ProjectionMutationKind,
    invalidate_relationship: bool,
) -> ModeledFixture {
    let todo_schema = todos();
    let user_schema = users();
    let descriptor = event_descriptor();
    let selector = crate::ProjectionEventSelector::try_from_descriptor(&descriptor).unwrap();
    let relationship = ProjectionRelationship::try_new("TodoView", "owner", "UserView").unwrap();
    let (relationship_effects, invalidations) = if invalidate_relationship {
        (
            vec![ProjectionRelationshipEffect::invalidate(
                0,
                relationship,
                vec![projection_key(0, "todo_id", "todo_id")],
            )
            .unwrap()],
            vec![ProjectionInvalidation::relationship("TodoView", "owner", "UserView").unwrap()],
        )
    } else {
        (
            vec![ProjectionRelationshipEffect::link(
                0,
                relationship,
                vec![projection_key(0, "todo_id", "todo_id")],
                vec![projection_key(0, "user_id", "owner_id")],
            )
            .unwrap()],
            vec![],
        )
    };
    let operation = ProjectionOperation::try_new(
        "upsert-todo",
        0,
        mutation_kind,
        ProjectionTarget::try_new("TodoView", "todos").unwrap(),
        vec![projection_key(0, "todo_id", "todo_id")],
        vec![
            projection_field(0, "todo_id", "todo_id"),
            projection_field(1, "owner_id", "owner_id"),
            projection_field(2, "title", "title"),
            projection_field(3, "status", "status"),
        ],
        relationship_effects,
        invalidations,
    )
    .unwrap();
    let arm = ProjectionArm::try_new("todo-state", selector, vec![operation]).unwrap();
    let program = ProjectionProgram::try_new(
        "projection-delta-test",
        1,
        ProjectionPartition::Expression(ProjectionExpression::constant(ProjectionValue::string(
            "tenant-a",
        ))),
        vec![arm],
    )
    .unwrap();
    let binding = ProjectionBinding::from_eventual_program(
        &program,
        ProjectionSourceBinding::try_new("todo-domain", "ordered-domain-events", 1).unwrap(),
        ProjectionOwner::try_new("projection-delta-test").unwrap(),
        execution,
        "distributed-projection-partition",
        PROJECTION_PARTITION_CODEC_VERSION,
        vec![ProjectionOutput::try_new("TodoView", "todos", todo_schema.clone()).unwrap()],
        vec![ProjectionRelationshipBinding::try_new("TodoView", "owner", "UserView").unwrap()],
        Some(ProjectionPhysicalTopology::from_protocol(
            &ProjectorTopologyId::new(1, "projection-delta-test", [0x44; 32]).unwrap(),
        )),
    )
    .unwrap();
    let catalog =
        crate::projection::catalog::ProjectionCatalog::try_new(vec![binding.clone()]).unwrap();
    let active = catalog
        .activate(
            vec![ProjectionBindingActivation::new(
                binding.id(),
                binding.program_id(),
                ProjectionEpoch::new("projection-delta-v1").unwrap(),
                state,
                Some(ProjectionExecutorRoute::local("delta-service").unwrap()),
            )],
            None,
        )
        .unwrap();
    let modeled = crate::graphql::SurfaceModeledProjection::try_from_catalog(
        program.clone(),
        &catalog,
        &active,
        binding.id(),
    )
    .unwrap();
    let surface = build_surface(&[todo_schema, user_schema], &SurfaceOptions::sqlite())
        .unwrap()
        .with_projectors([SurfaceProjector::new("projection-delta-test").modeled(modeled)])
        .unwrap();
    ModeledFixture { surface, program }
}

fn selected_export(surface: &crate::graphql::Surface) -> DistributedClientSurfaceExport {
    let selected = surface_for_role(
        surface,
        "delta-user",
        &BTreeMap::from([
            ("TodoView".into(), RoleGrant::all_columns()),
            ("UserView".into(), RoleGrant::all_columns()),
        ]),
    )
    .unwrap();
    DistributedClientSurfaceExport::from_selected("delta-service", selected).unwrap()
}

fn selected_export_with_embedded_target(
    surface: &crate::graphql::Surface,
) -> DistributedClientSurfaceExport {
    let selected = surface_for_role(
        surface,
        "delta-user",
        &BTreeMap::from([
            ("TodoView".into(), RoleGrant::all_columns()),
            ("UserView".into(), RoleGrant::columns(["display_name"])),
        ]),
    )
    .unwrap();
    DistributedClientSurfaceExport::from_selected("delta-service", selected).unwrap()
}

fn selected_export_with_embedded_source(
    surface: &crate::graphql::Surface,
) -> DistributedClientSurfaceExport {
    let selected = surface_for_role(
        surface,
        "delta-user",
        &BTreeMap::from([
            (
                "TodoView".into(),
                RoleGrant::columns(["owner_id", "status", "title"]),
            ),
            ("UserView".into(), RoleGrant::all_columns()),
        ]),
    )
    .unwrap();
    DistributedClientSurfaceExport::from_selected("delta-service", selected).unwrap()
}

fn projection_key(ordinal: u32, name: &str, body_field: &str) -> ProjectionKeyField {
    ProjectionKeyField::try_new(
        ordinal,
        name,
        ProjectionExpression::body_path(ProjectionValueType::String, [body_field]).unwrap(),
    )
    .unwrap()
}

fn projection_field(ordinal: u32, name: &str, body_field: &str) -> ProjectionField {
    ProjectionField::try_new(
        ordinal,
        name,
        ProjectionAssignment::Set(
            ProjectionExpression::body_path(ProjectionValueType::String, [body_field]).unwrap(),
        ),
    )
    .unwrap()
}

fn event_descriptor() -> DomainEventDescriptor {
    DomainEventDescriptor {
        name: Cow::Borrowed("todo.state-published"),
        version: 1,
        body: DomainEventBodyDescriptor {
            kind: DomainEventBodyKind::State,
            type_name: Cow::Borrowed("TodoState"),
            version: 1,
            schema: Cow::Borrowed("urn:distributed:test:projection-delta:v1"),
            fingerprint: Cow::Borrowed(BODY_FINGERPRINT),
            codec: Cow::Borrowed(DOMAIN_EVENT_BODY_CODEC),
            codec_version: DOMAIN_EVENT_BODY_CODEC_VERSION,
        },
    }
}

#[derive(Serialize)]
struct StateBody<'a> {
    todo_id: &'a str,
    owner_id: &'a str,
    title: &'a str,
    status: &'a str,
}

fn state_occurrence(sequence: u64, todo_id: &str, title: &str) -> DomainEventOccurrence {
    DomainEventOccurrence::capture(
        event_descriptor(),
        DomainEventEnvelope {
            aggregate_type: "todo".into(),
            aggregate_id: todo_id.into(),
            aggregate_sequence: sequence,
            publication_ordinal: 0,
            occurred_at: UNIX_EPOCH + Duration::from_secs(sequence),
            metadata: BTreeMap::new(),
        },
        &StateBody {
            todo_id,
            owner_id: "owner-secret",
            title,
            status: "open",
        },
    )
    .unwrap()
}

fn partial_state_occurrence(sequence: u64, todo_id: &str) -> DomainEventOccurrence {
    DomainEventOccurrence::capture(
        event_descriptor(),
        DomainEventEnvelope {
            aggregate_type: "todo".into(),
            aggregate_id: todo_id.into(),
            aggregate_sequence: sequence,
            publication_ordinal: 0,
            occurred_at: UNIX_EPOCH + Duration::from_secs(sequence),
            metadata: BTreeMap::new(),
        },
        &json!({
            "todo_id": todo_id,
            "owner_id": "owner-secret",
            "status": "open"
        }),
    )
    .unwrap()
}

fn todos() -> TableSchema {
    TableSchema {
        model_name: "TodoView".into(),
        table_name: "todos".into(),
        columns: vec![
            primary_column("todo_id"),
            column("owner_id"),
            column("title"),
            column("status"),
        ],
        primary_key: PrimaryKey::new(["todo_id"]),
        version_column: Some("_sourced_version".into()),
        foreign_keys: vec![],
        indexes: vec![],
        relationships: vec![RelationshipDef {
            field_name: "owner".into(),
            kind: RelationshipKind::BelongsTo,
            target_model: "UserView".into(),
            foreign_key: Some("owner_id".into()),
            through: None,
            target_foreign_key: None,
        }],
        kind: TableKind::ReadModel,
    }
}

fn users() -> TableSchema {
    TableSchema {
        model_name: "UserView".into(),
        table_name: "users".into(),
        columns: vec![primary_column("user_id"), column("display_name")],
        primary_key: PrimaryKey::new(["user_id"]),
        version_column: Some("_sourced_version".into()),
        foreign_keys: vec![],
        indexes: vec![],
        relationships: vec![],
        kind: TableKind::ReadModel,
    }
}

fn column(name: &str) -> TableColumn {
    TableColumn::new(name, name, ColumnType::Text)
}

fn primary_column(name: &str) -> TableColumn {
    TableColumn {
        primary_key: true,
        ..column(name)
    }
}

const TEST_CACHE_SCOPE_TOKEN: &str = "v1.cache-scope.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const TEST_PARTITION_TOKEN: &str =
    "v1.projection-partition.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

struct TestRequestAuthority {
    calls: Cell<usize>,
    generation: String,
    principal: crate::command_ledger::PrincipalPartitionId,
    cache_scope: ProjectionDeltaCacheScopeToken,
    causation: crate::command_ledger::CausationId,
    record: AuthorizationTransition,
    relationship: AuthorizationTransition,
    partition_result: PartitionResult,
}

#[derive(Clone, Copy)]
enum PartitionResult {
    Opaque,
    Unit,
}

impl TestRequestAuthority {
    fn authorized(generation: &str) -> Self {
        Self::new(generation, authorized_transition(), authorized_transition())
    }

    fn new(
        generation: &str,
        record: AuthorizationTransition,
        relationship: AuthorizationTransition,
    ) -> Self {
        Self {
            calls: Cell::new(0),
            generation: generation.into(),
            principal: crate::command_ledger::PrincipalPartitionId::new("principal-scope-9")
                .unwrap(),
            cache_scope: ProjectionDeltaCacheScopeToken::parse_wire(TEST_CACHE_SCOPE_TOKEN)
                .unwrap(),
            causation: crate::command_ledger::CausationId::new(),
            record,
            relationship,
            partition_result: PartitionResult::Opaque,
        }
    }
}

impl ProjectionPartitionScopeEncoder for TestRequestAuthority {
    fn encode(
        &self,
        authority: ProjectionPartitionAuthority<'_>,
        partition: &crate::ResolvedProjectionPartition,
    ) -> Result<Option<ProjectionDeltaPartition>, ProjectionDeltaError> {
        self.calls.set(self.calls.get() + 1);
        assert_eq!(
            authority.surface,
            &ProjectionDeltaSurfaceIdentity::Role {
                name: "delta-user".into()
            }
        );
        assert!(!authority.authorization_generation.is_empty());
        assert!(!authority.principal_scope.as_str().is_empty());
        assert_eq!(authority.cache_scope, &self.cache_scope);
        assert!(!partition.canonical_bytes().is_empty());
        Ok(Some(match self.partition_result {
            PartitionResult::Opaque => ProjectionDeltaPartition::Opaque {
                token: TEST_PARTITION_TOKEN.into(),
            },
            PartitionResult::Unit => ProjectionDeltaPartition::Unit,
        }))
    }
}

impl ProjectionVisibilityEvaluator for TestRequestAuthority {
    fn record_transition(
        &self,
        _source: ProjectionMutationSource,
        _mutation: &ResolvedProjectionMutation,
    ) -> Result<AuthorizationTransition, ProjectionDeltaError> {
        Ok(self.record)
    }

    fn relationship_transition(
        &self,
        _source: ProjectionMutationSource,
        _effect: &ResolvedProjectionRelationshipEffect,
    ) -> Result<AuthorizationTransition, ProjectionDeltaError> {
        Ok(self.relationship)
    }
}

impl ProjectionDeltaRequestAuthority for TestRequestAuthority {
    fn authorization_generation(&self) -> &str {
        &self.generation
    }

    fn principal_scope(&self) -> &crate::command_ledger::PrincipalPartitionId {
        &self.principal
    }

    fn cache_scope(&self) -> &ProjectionDeltaCacheScopeToken {
        &self.cache_scope
    }

    fn command_causation_id(&self) -> &crate::command_ledger::CausationId {
        &self.causation
    }
}

const fn authorized_transition() -> AuthorizationTransition {
    AuthorizationTransition {
        before: ProjectionDeltaVisibility::Authorized,
        after: ProjectionDeltaVisibility::Authorized,
    }
}

fn empty_delta() -> ProjectionDelta {
    ProjectionDelta {
        wire_version: PROJECTION_DELTA_WIRE_VERSION,
        identity: ProjectionDeltaIdentity {
            manifest_version: crate::graphql::client_manifest::DISTRIBUTED_CLIENT_MANIFEST_VERSION,
            client_protocol_version:
                crate::graphql::client_manifest::DISTRIBUTED_CLIENT_PROTOCOL_VERSION,
            surface: ProjectionDeltaSurfaceIdentity::Role {
                name: "delta-user".into(),
            },
            schema_fingerprint: "sha256:schema".into(),
            protocol_fingerprint: "sha256:protocol".into(),
            authorization_generation: "generation-1".into(),
            cache_scope_token: TEST_CACHE_SCOPE_TOKEN.into(),
            command_causation_id: "cause-1".into(),
        },
        projections: vec![],
        occurrences: vec![],
        operations: vec![],
        recoveries: vec![],
    }
}

fn operation(
    occurrence_ordinal: u32,
    mutation: ProjectionDeltaMutation,
) -> ProjectionDeltaOperation {
    ProjectionDeltaOperation {
        occurrence_ordinal,
        projection_refs: vec![0],
        mutation,
    }
}

fn test_scope(_partition: &str, model: &str, key: Vec<DeltaKeyField>) -> ProjectionDeltaScope {
    ProjectionDeltaScope {
        partition: ProjectionDeltaPartition::Opaque {
            token: TEST_PARTITION_TOKEN.into(),
        },
        model: model.into(),
        key,
    }
}

fn key_field(ordinal: u32, field: &str, value: &str) -> DeltaKeyField {
    DeltaKeyField {
        ordinal,
        field: field.into(),
        value: DeltaValue::String(value.into()),
    }
}
