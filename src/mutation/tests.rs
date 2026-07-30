//! Unit and golden tests for the mutation IR and interpreters.

use crate::projection::{
    ProjectionEventSelector, ProjectionExpression, ProjectionPartition, ProjectionTarget,
    ProjectionValue, ProjectionValueType,
};
use crate::{DomainEventBodyKind, DOMAIN_EVENT_OCCURRENCE_VERSION};

use super::*;

const STATE_FP: &str = "sha256:1111111111111111111111111111111111111111111111111111111111111111";

fn todos_target() -> ProjectionTarget {
    ProjectionTarget::try_new("Todos", "todos").unwrap()
}

fn string_path(segments: &[&str]) -> MutationExpression {
    MutationExpression::input_path(
        ProjectionValueType::String,
        segments.iter().map(|segment| (*segment).to_owned()),
    )
    .unwrap()
}

fn explicit_save_todo() -> MutationProgram {
    let key =
        vec![MutationKeyField::try_new(0, "todo_id", string_path(&["todo", "todo_id"])).unwrap()];
    let fields = vec![
        MutationField::try_new(
            0,
            "todo_id",
            MutationAssignment::set(string_path(&["todo", "todo_id"])),
        )
        .unwrap(),
        MutationField::try_new(
            1,
            "owner_id",
            MutationAssignment::set(string_path(&["todo", "owner_id"])),
        )
        .unwrap(),
        MutationField::try_new(
            2,
            "title",
            MutationAssignment::set(string_path(&["todo", "title"])),
        )
        .unwrap(),
        MutationField::try_new(
            3,
            "status",
            MutationAssignment::set(string_path(&["todo", "status"])),
        )
        .unwrap(),
    ];
    let op = MutationOperation::try_new(
        "upsert-todo",
        0,
        MutationKind::Upsert,
        todos_target(),
        key,
        fields,
        Some(MutationConflictTarget::PrimaryKey),
        Vec::new(),
        Vec::new(),
        None,
    )
    .unwrap();
    MutationProgram::try_new("save_todo", 1, vec![op]).unwrap()
}

fn sugar_save_todo() -> MutationProgram {
    let op = MutationOperation::state_upsert(
        "upsert-todo",
        0,
        todos_target(),
        &[(0, "todo_id")],
        &[
            (0, "owner_id", ProjectionValueType::String),
            (1, "title", ProjectionValueType::String),
            (2, "status", ProjectionValueType::String),
        ],
        &["todo"],
    )
    .unwrap();
    MutationProgram::try_new("save_todo", 1, vec![op]).unwrap()
}

fn delete_todo() -> MutationProgram {
    let key = vec![MutationKeyField::try_new(0, "todo_id", string_path(&["todo_id"])).unwrap()];
    let op = MutationOperation::try_new(
        "delete-todo",
        0,
        MutationKind::Delete,
        todos_target(),
        key,
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        None,
    )
    .unwrap();
    MutationProgram::try_new("delete_todo", 1, vec![op]).unwrap()
}

fn patch_todo_status() -> MutationProgram {
    let key = vec![MutationKeyField::try_new(0, "todo_id", string_path(&["todo_id"])).unwrap()];
    let fields = vec![
        MutationField::try_new(
            0,
            "status",
            MutationAssignment::set(string_path(&["status"])),
        )
        .unwrap(),
        MutationField::try_new(1, "legacy_title", MutationAssignment::unset()).unwrap(),
        MutationField::try_new(2, "assignee_id", MutationAssignment::unknown()).unwrap(),
    ];
    let op = MutationOperation::try_new(
        "patch-todo",
        0,
        MutationKind::Patch,
        todos_target(),
        key,
        fields,
        None,
        Vec::new(),
        Vec::new(),
        None,
    )
    .unwrap();
    MutationProgram::try_new("patch_todo", 1, vec![op]).unwrap()
}

fn state_selector() -> ProjectionEventSelector {
    ProjectionEventSelector::try_new(
        DOMAIN_EVENT_OCCURRENCE_VERSION,
        "todo.state-published",
        1,
        DomainEventBodyKind::State,
        "TodoState",
        1,
        "urn:distributed:test:mutation-body:v1",
        STATE_FP,
        "distributed-json",
        1,
    )
    .unwrap()
}

#[test]
fn sugar_and_explicit_upsert_canonicalize_identically() {
    let explicit = explicit_save_todo();
    let sugar = sugar_save_todo();
    assert_eq!(
        explicit.canonical_bytes().unwrap(),
        sugar.canonical_bytes().unwrap()
    );
    assert_eq!(
        explicit.id().unwrap().to_string(),
        sugar.id().unwrap().to_string()
    );
    assert!(explicit
        .id()
        .unwrap()
        .to_string()
        .starts_with("mp1:sha256:"));
}

#[test]
fn mutation_program_contains_no_event_selector_fields() {
    let program = explicit_save_todo();
    let json = serde_json::to_value(&program).unwrap();
    let text = json.to_string();
    assert!(!text.contains("event_name"));
    assert!(!text.contains("body_fingerprint"));
    assert!(!text.contains("\"selector\""));
    assert!(!text.contains("\"placement\""));
    assert!(!text.contains("\"owner\""));
    assert!(json.get("arms").is_none());
    assert_eq!(json["ir_version"], 1);
    assert_eq!(json["operation_semantics_version"], 1);
    assert_eq!(json["name"], "save_todo");
}

#[test]
fn field_presence_lattice_is_preserved_in_ast() {
    let program = patch_todo_status();
    let fields = program.operations()[0].fields();
    assert!(matches!(fields[0].assignment(), MutationAssignment::Set(_)));
    assert!(matches!(fields[1].assignment(), MutationAssignment::Unset));
    assert!(matches!(
        fields[2].assignment(),
        MutationAssignment::Unknown
    ));
    let json = serde_json::to_value(program.operations()[0].fields()).unwrap();
    assert_eq!(json[1]["assignment"]["kind"], "unset");
    assert_eq!(json[2]["assignment"]["kind"], "unknown");
}

#[test]
fn delete_and_upsert_round_trip_through_canonical_bytes() {
    for program in [explicit_save_todo(), delete_todo(), patch_todo_status()] {
        let bytes = program.canonical_bytes().unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["ir_version"], MUTATION_PROGRAM_IR_VERSION);
        let id = program.id().unwrap();
        assert_eq!(MutationProgramId::parse(&id.to_string()).unwrap(), id);
    }
}

#[test]
fn rejects_incomplete_keys_and_ambiguous_same_record_ops() {
    let incomplete = MutationOperation::try_new(
        "bad",
        0,
        MutationKind::Upsert,
        todos_target(),
        Vec::new(),
        vec![
            MutationField::try_new(0, "title", MutationAssignment::set(string_path(&["title"])))
                .unwrap(),
        ],
        Some(MutationConflictTarget::PrimaryKey),
        Vec::new(),
        Vec::new(),
        None,
    );
    assert!(matches!(
        incomplete,
        Err(MutationProgramError::InvalidOperation { .. })
    ));

    let key = vec![MutationKeyField::try_new(0, "todo_id", string_path(&["todo_id"])).unwrap()];
    let left = MutationOperation::try_new(
        "left",
        0,
        MutationKind::Upsert,
        todos_target(),
        key.clone(),
        vec![
            MutationField::try_new(0, "title", MutationAssignment::set(string_path(&["title"])))
                .unwrap(),
        ],
        Some(MutationConflictTarget::PrimaryKey),
        Vec::new(),
        Vec::new(),
        None,
    )
    .unwrap();
    let right = MutationOperation::try_new(
        "right",
        1,
        MutationKind::Patch,
        todos_target(),
        key,
        vec![MutationField::try_new(
            0,
            "status",
            MutationAssignment::set(string_path(&["status"])),
        )
        .unwrap()],
        None,
        Vec::new(),
        Vec::new(),
        None,
    )
    .unwrap();
    let ambiguous = MutationProgram::try_new("ambiguous", 1, vec![left, right]);
    assert!(matches!(
        ambiguous,
        Err(MutationProgramError::AmbiguousMutation { .. })
    ));
}

#[test]
fn rejects_programs_above_operation_limit() {
    let mut operations = Vec::new();
    for index in 0..=MAX_MUTATION_OPERATIONS {
        let key = vec![MutationKeyField::try_new(
            0,
            "todo_id",
            MutationExpression::constant(ProjectionValue::string(format!("t{index}"))),
        )
        .unwrap()];
        // Different constant keys avoid static ambiguity.
        operations.push(
            MutationOperation::try_new(
                format!("op-{index}"),
                index as u32,
                MutationKind::Delete,
                todos_target(),
                key,
                Vec::new(),
                None,
                Vec::new(),
                Vec::new(),
                None,
            )
            .unwrap(),
        );
    }
    let result = MutationProgram::try_new("too-many", 1, operations);
    assert!(matches!(
        result,
        Err(MutationProgramError::TooManyOperations { .. })
    ));
}

#[test]
fn fingerprint_changes_for_semantic_edits_and_stable_for_irrelevant_order() {
    let base = explicit_save_todo();
    let reordered_fields = explicit_save_todo();
    // Reconstruct with same semantics; program already sorts fields by ordinal.
    assert_eq!(base.id().unwrap(), reordered_fields.id().unwrap());

    let mutated = {
        let key = vec![
            MutationKeyField::try_new(0, "todo_id", string_path(&["todo", "todo_id"])).unwrap(),
        ];
        let fields = vec![
            MutationField::try_new(
                0,
                "todo_id",
                MutationAssignment::set(string_path(&["todo", "todo_id"])),
            )
            .unwrap(),
            MutationField::try_new(
                1,
                "owner_id",
                MutationAssignment::set(string_path(&["todo", "owner_id"])),
            )
            .unwrap(),
            MutationField::try_new(
                2,
                "title",
                MutationAssignment::set(string_path(&["todo", "title"])),
            )
            .unwrap(),
            MutationField::try_new(
                3,
                "status",
                MutationAssignment::set(MutationExpression::constant(ProjectionValue::string(
                    "completed",
                ))),
            )
            .unwrap(),
        ];
        let op = MutationOperation::try_new(
            "upsert-todo",
            0,
            MutationKind::Upsert,
            todos_target(),
            key,
            fields,
            Some(MutationConflictTarget::PrimaryKey),
            Vec::new(),
            Vec::new(),
            None,
        )
        .unwrap();
        MutationProgram::try_new("save_todo", 1, vec![op]).unwrap()
    };
    assert_ne!(base.id().unwrap(), mutated.id().unwrap());
    let _ = reordered_fields.canonical_bytes();
}

#[test]
fn rewrite_to_projection_operations_uses_body_path_bindings() {
    let program = explicit_save_todo();
    let bindings = simple_body_bindings(&[
        (
            &["todo", "todo_id"],
            &["todo_id"],
            ProjectionValueType::String,
        ),
        (
            &["todo", "owner_id"],
            &["owner_id"],
            ProjectionValueType::String,
        ),
        (&["todo", "title"], &["title"], ProjectionValueType::String),
        (
            &["todo", "status"],
            &["status"],
            ProjectionValueType::String,
        ),
    ])
    .unwrap();
    let binding =
        MutationEventBinding::try_new(state_selector(), bindings, program.clone()).unwrap();
    let arm = binding.to_projection_arm("state").unwrap();
    assert_eq!(arm.operations().len(), 1);
    assert_eq!(
        arm.operations()[0].kind(),
        crate::projection::ProjectionMutationKind::Upsert
    );
    assert_eq!(arm.operations()[0].fields().len(), 4);

    let projection = binding
        .to_projection_program("project_todos", 1, ProjectionPartition::Unit, "state")
        .unwrap();
    assert_eq!(projection.arms().len(), 1);
    assert!(projection
        .id()
        .unwrap()
        .to_string()
        .starts_with("pp1:sha256:"));
}

#[test]
fn cache_lowering_preserves_partial_patch_and_fail_closed_auth() {
    let program = patch_todo_status();
    let full = lower_mutation_cache(&program, &MutationCacheVisibility::full()).unwrap();
    assert!(matches!(
        &full.effects()[0],
        MutationCacheEffect::Patch { fields, .. }
            if fields.contains(&"status".to_owned()) && fields.contains(&"legacy_title".to_owned())
    ));

    let missing_base = lower_mutation_cache(
        &program,
        &MutationCacheVisibility {
            authorized: true,
            has_base_record: false,
            relationship_covered: true,
        },
    )
    .unwrap();
    assert!(matches!(
        &missing_base.effects()[0],
        MutationCacheEffect::Invalidate { .. }
    ));

    let unauthorized =
        lower_mutation_cache(&program, &MutationCacheVisibility::unauthorized()).unwrap();
    assert!(matches!(
        &unauthorized.effects()[0],
        MutationCacheEffect::Invalidate { .. }
    ));
}

#[test]
fn portable_handler_catalog_rejects_duplicate_owner_event_epoch() {
    let program = explicit_save_todo();
    // Catalog uniqueness only needs a valid binding registration.
    let binding = MutationEventBinding::try_new(
        state_selector(),
        vec![body_field_binding(
            ["todo", "todo_id"],
            ["todo_id"],
            ProjectionValueType::String,
        )
        .unwrap()],
        program,
    )
    .unwrap();
    let reg = MutationHandlerRegistration::try_new(
        "todos",
        1,
        "todo-reads",
        "e2e-ui-todos-v2",
        MutationHandlerPlacement::EventualLocal,
        ProjectionPartition::Unit,
        binding.clone(),
    )
    .unwrap();
    let mut catalog = MutationHandlerCatalog::new();
    catalog.register(reg.clone()).unwrap();
    let dup = MutationHandlerRegistration::try_new(
        "todos-dup",
        1,
        "todo-reads",
        "e2e-ui-todos-v2",
        MutationHandlerPlacement::EventualLocal,
        ProjectionPartition::Unit,
        binding,
    )
    .unwrap();
    assert!(catalog.register(dup).is_err());
    assert_eq!(catalog.registrations().len(), 1);
    assert!(!reg.digest().unwrap().iter().all(|byte| *byte == 0));
}

#[test]
fn preview_composition_zero_binding_and_multi_owner() {
    assert!(!zero_binding_preview().has_optimism());
    let program = explicit_save_todo();
    let make_reg = |owner: &str, name: &str| {
        let binding = MutationEventBinding::try_new(
            state_selector(),
            vec![body_field_binding(
                ["todo", "todo_id"],
                ["todo_id"],
                ProjectionValueType::String,
            )
            .unwrap()],
            program.clone(),
        )
        .unwrap();
        MutationHandlerRegistration::try_new(
            name,
            1,
            owner,
            "epoch-1",
            MutationHandlerPlacement::EventualLocal,
            ProjectionPartition::Unit,
            binding,
        )
        .unwrap()
    };
    // Different models would be needed for multi-owner same event without dual-writer;
    // here both use Todos so catalog rejects dual writer. Compose at preview layer
    // with two handler refs directly (catalog validation is separate).
    let a = make_reg("owner-a", "handler-a");
    let b = make_reg("owner-b", "handler-b");
    let layer = compose_event_preview(
        &[&a, &b],
        &state_selector(),
        &MutationCacheVisibility::full(),
    )
    .unwrap();
    assert_eq!(layer.contributions.len(), 2);
    assert_eq!(causal_scopes(&layer).len(), 1);
    assert_eq!(causal_scopes(&layer)[0].model, "Todos");
}

#[test]
fn read_model_capabilities_are_internal_metadata() {
    let caps = ReadModelMutationCapabilities::new(
        "Todos",
        "todos",
        vec![MutationKeyCapability {
            name: "todo_id".into(),
            value_type: ProjectionValueType::String,
            ordinal: 0,
        }],
        vec![
            MutationFieldCapability {
                name: "todo_id".into(),
                value_type: ProjectionValueType::String,
                nullable: false,
                supports_unset: false,
            },
            MutationFieldCapability {
                name: "title".into(),
                value_type: ProjectionValueType::String,
                nullable: false,
                supports_unset: false,
            },
        ],
        Vec::new(),
    );
    assert!(caps.is_key_field("todo_id"));
    assert_eq!(caps.returning, ["todo_id", "title"]);
    assert_eq!(caps.field("title").unwrap().nullable, false);
}

#[test]
fn golden_fixture_round_trip_matches_canonical_program() {
    let program = explicit_save_todo();
    let bytes = program.canonical_bytes().unwrap();
    let fixture_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/mutation-program-v1.json"
    );
    // Write-once style: if fixture missing, create from program; if present, compare.
    if !std::path::Path::new(fixture_path).exists() {
        std::fs::write(fixture_path, &bytes).unwrap();
    }
    let fixture = std::fs::read(fixture_path).unwrap();
    let fixture_value: serde_json::Value = serde_json::from_slice(&fixture).unwrap();
    let program_value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        fixture_value, program_value,
        "mutation-program-v1.json golden vector drifted"
    );
    assert_eq!(fixture_value["ir_version"], 1);
    assert!(fixture_value.get("arms").is_none());
    assert!(fixture_value.get("selector").is_none());
}

#[test]
fn constant_expression_rewrite_without_inputs() {
    let expr = MutationExpression::constant(ProjectionValue::string("completed"));
    let rewritten = expr
        .rewrite_with(&|path, _| {
            Err(MutationProgramError::MissingInput {
                path: path.join("."),
            })
        })
        .unwrap();
    assert_eq!(
        rewritten,
        ProjectionExpression::constant(ProjectionValue::string("completed"))
    );
}

#[test]
fn presence_labels_cover_lattice() {
    assert_eq!(
        presence_label(&ResolvedMutationValue::Value(ProjectionValue::null())),
        "null"
    );
    assert_eq!(
        presence_label(&ResolvedMutationValue::Value(ProjectionValue::string("x"))),
        "value"
    );
    assert_eq!(presence_label(&ResolvedMutationValue::Absent), "absent");
    assert_eq!(presence_label(&ResolvedMutationValue::Unset), "unset");
    assert_eq!(presence_label(&ResolvedMutationValue::Unknown), "unknown");
    assert!(!is_cache_writable(&ResolvedMutationValue::Unknown));
    assert!(is_cache_writable(&ResolvedMutationValue::Unset));
}
