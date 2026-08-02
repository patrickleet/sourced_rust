use super::*;
use crate::graphql::{GraphqlInputType, GraphqlOutputType, GraphqlTypeDef, GraphqlTypeField};
use crate::microsvc::Session;
use crate::outbox::OutboxMessage;
use crate::projection_protocol::{
    ProjectionPartitionSpec, ProjectorTopologyId, ResolvedProjectionObligation,
};
use crate::table::{ColumnType, PrimaryKey, TableColumn, TableKind, TableSchema};
use serde::{Deserialize, Serialize};
use std::any::TypeId;

#[allow(dead_code)]
#[derive(Deserialize)]
struct Input {
    id: String,
}

impl GraphqlInputType for Input {
    fn graphql_type() -> GraphqlTypeDef {
        GraphqlTypeDef::new(
            "Input",
            vec![GraphqlTypeField {
                name: "id".into(),
                type_name: "String".into(),
                nullable: false,
                list: false,
                item_nullable: false,
                nested: None,
            }],
        )
        .with_type_id(TypeId::of::<Self>())
    }
}

#[derive(Serialize)]
struct Payload {
    id: String,
}

#[derive(Clone, Serialize, Deserialize, crate::ReadModel)]
#[readmodel(table = "projected_owners", primary_key = ["id"])]
struct ProjectedOwner {
    #[readmodel(id)]
    id: String,
}

#[derive(Clone, Serialize, Deserialize, crate::ReadModel)]
#[readmodel(table = "projected_rows", primary_key = ["id"])]
struct ProjectedRow {
    #[readmodel(id)]
    id: String,
    count: i64,
    note: Option<String>,
    owner_id: String,
    #[readmodel(belongs_to = "ProjectedOwner", foreign_key = "owner_id")]
    owner: Option<ProjectedOwner>,
}

#[derive(Serialize, crate::DomainEvent)]
#[domain_event(name = "todo.completed", version = 1)]
struct TodoCompleted {
    todo_id: String,
    status: String,
}

#[derive(Serialize, crate::DomainEvent)]
#[domain_event(name = "todo.completed", version = 1)]
struct ConflictingTodoCompleted {
    todo_id: String,
    completed_at: String,
}

#[derive(Serialize, crate::DomainEvent)]
#[domain_event(name = "todo.renamed", version = 1)]
struct TodoRenamed {
    todo_id: String,
    title: String,
}

#[derive(Serialize, crate::DomainState)]
#[domain_state(version = 1)]
struct TodoState {
    todo_id: String,
    status: String,
}

enum DishonestEventContract {}

impl crate::domain_event::DomainEventContract for DishonestEventContract {
    const EVENT_NAME: &'static str = "todo.lied";
    const EVENT_VERSION: u64 = 1;

    fn descriptor() -> crate::DomainEventDescriptor {
        <TodoCompleted as crate::DomainEvent>::DESCRIPTOR.clone()
    }
}

impl crate::domain_event::DomainEventBodyContract<TodoCompleted> for DishonestEventContract {}

enum DishonestStateContract {}

impl crate::domain_event::DomainEventContract for DishonestStateContract {
    const EVENT_NAME: &'static str = "todo.completed";
    const EVENT_VERSION: u64 = 1;

    fn descriptor() -> crate::DomainEventDescriptor {
        <TodoCompleted as crate::DomainEvent>::DESCRIPTOR.clone()
    }
}

impl crate::domain_event::DomainEventBodyContract<TodoState> for DishonestStateContract {}

impl GraphqlOutputType for Payload {
    fn graphql_type() -> GraphqlTypeDef {
        GraphqlTypeDef::new(
            "Payload",
            vec![GraphqlTypeField {
                name: "id".into(),
                type_name: "String".into(),
                nullable: false,
                list: false,
                item_nullable: false,
                nested: None,
            }],
        )
        .with_type_id(TypeId::of::<Self>())
    }
}

#[test]
fn preparation_serializes_and_retains_the_typed_payload_until_commit() {
    let prepared = PreparedCommand::<Eventual<Payload>>::prepare(Payload {
        id: "todo-1".into(),
    })
    .unwrap();
    assert_eq!(prepared.consistency(), CommandConsistency::Eventual);
    assert_eq!(prepared.serialized_payload()["id"], "todo-1");
    let (committed, serialized) = prepared.finalize_after_commit();
    assert_eq!(committed.payload().id, "todo-1");
    assert_eq!(serialized["id"], "todo-1");
}

#[test]
fn projected_output_is_generated_from_relational_schema_without_graphql_output_derive() {
    let contract = typed_command::<Input, Atomic<ProjectedRow>>("row.project").into_contract();

    assert_eq!(contract.output.name, "ProjectedRow");
    assert_eq!(
        contract
            .output
            .fields
            .iter()
            .map(|field| (
                field.name.as_str(),
                field.type_name.as_str(),
                field.nullable
            ))
            .collect::<Vec<_>>(),
        vec![
            ("id", "String", false),
            ("count", "BigInt", false),
            ("note", "String", true),
            ("owner_id", "String", false),
        ]
    );
    assert_eq!(contract.output.type_id, Some(TypeId::of::<ProjectedRow>()));
}

#[test]
fn successful_consistency_wire_vocabulary_is_exact_and_breaking() {
    let cases = [
        (CommandConsistency::Succeeded, "\"succeeded\""),
        (CommandConsistency::Eventual, "\"causal\""),
        (CommandConsistency::Atomic, "\"projected\""),
    ];

    for (consistency, encoded) in cases {
        assert_eq!(serde_json::to_string(&consistency).unwrap(), encoded);
        assert_eq!(
            serde_json::from_str::<CommandConsistency>(encoded).unwrap(),
            consistency
        );
    }
    assert!(serde_json::from_str::<CommandConsistency>("\"accepted\"").is_err());
    assert!(serde_json::from_str::<CommandConsistency>("\"fact\"").is_err());
}

#[test]
fn command_events_are_exact_values_independent_of_projector_declarations() {
    let contract = typed_command::<Input, Succeeded<Payload>>("todo.complete")
        .emits(crate::events![TodoCompleted])
        .into_contract();
    let binding =
        TypedServiceCommandBinding::from_contracts("todos", std::slice::from_ref(&contract));
    assert!(binding.is_ok());
    assert_eq!(contract.projections.selectors.len(), 1);
    assert_eq!(
        contract.projections.selectors[0].event_name(),
        "todo.completed"
    );
}

#[test]
fn command_event_registration_rejects_duplicates_and_conflicting_schemas() {
    let duplicate = typed_command::<Input, Succeeded<Payload>>("todo.duplicate")
        .emits(crate::events![TodoCompleted])
        .emits(crate::events![TodoCompleted])
        .into_contract();
    assert!(
        TypedServiceCommandBinding::from_contracts("todos", &[duplicate])
            .unwrap_err()
            .contains("repeats an exact emitted")
    );

    let conflicting = typed_command::<Input, Succeeded<Payload>>("todo.conflicting")
        .emits(crate::events![TodoCompleted, ConflictingTodoCompleted])
        .into_contract();
    assert!(
        TypedServiceCommandBinding::from_contracts("todos", &[conflicting])
            .unwrap_err()
            .contains("conflicting schemas")
    );
}

#[test]
fn command_registration_distrusts_manual_event_contracts() {
    let mismatched_name = typed_command::<Input, Succeeded<Payload>>("todo.dishonest-event")
        .emits(crate::events![DishonestEventContract])
        .into_contract();
    assert!(
        TypedServiceCommandBinding::from_contracts("todos", &[mismatched_name])
            .unwrap_err()
            .contains("differs from descriptor name")
    );

    let mismatched_state = typed_command::<Input, Succeeded<Payload>>("todo.dishonest-state")
        .emits(crate::events![DishonestStateContract])
        .preview(crate::state_preview! {
            DishonestStateContract => TodoState {
                todo_id: input.id,
                ..unknown
            }
        })
        .into_contract();
    assert!(
        TypedServiceCommandBinding::from_contracts("todos", &[mismatched_state])
            .unwrap_err()
            .contains("does not exactly describe")
    );
}

#[test]
fn command_preview_requires_membership_and_rejects_server_only_sources() {
    let outside = typed_command::<Input, Succeeded<Payload>>("todo.outside")
        .emits(crate::events![TodoCompleted])
        .preview(crate::event_preview! {
            TodoRenamed => TodoRenamed {
                todo_id: input.id,
                ..unknown
            }
        })
        .into_contract();
    assert!(
        TypedServiceCommandBinding::from_contracts("todos", &[outside])
            .unwrap_err()
            .contains("outside its exact emitted event set")
    );

    let server_only = typed_command::<Input, Succeeded<Payload>>("todo.server-only")
        .emits(crate::events![TodoCompleted])
        .preview(
            CommandProjectionPreview::new()
                .events(crate::events![TodoCompleted])
                .field(["todo_id"], CommandProjectionPreviewSource::ServerOnly),
        )
        .into_contract();
    assert!(
        TypedServiceCommandBinding::from_contracts("todos", &[server_only])
            .unwrap_err()
            .contains("server-only preview provenance")
    );
}

#[test]
fn partial_preview_retains_known_unknown_and_typed_constant_sources() {
    let contract = typed_command::<Input, Succeeded<Payload>>("todo.partial")
        .emits(crate::events![TodoCompleted])
        .preview(crate::event_preview! {
            TodoCompleted => TodoCompleted {
                todo_id: input.id,
                status: "completed",
                ..unknown
            }
        })
        .into_contract();
    TypedServiceCommandBinding::from_contracts("todos", std::slice::from_ref(&contract)).unwrap();
    assert_eq!(contract.projections.previews[0].preview.fields.len(), 2);
}

#[test]
fn repeated_preview_declarations_preserve_synthetic_occurrence_order() {
    let contract = typed_command::<Input, Succeeded<Payload>>("todo.repeated-preview")
        .emits(crate::events![TodoCompleted])
        .preview(crate::event_preview! {
            TodoCompleted => TodoCompleted {
                todo_id: "first",
                ..unknown
            }
        })
        .preview(crate::event_preview! {
            TodoCompleted => TodoCompleted {
                todo_id: input.id,
                ..unknown
            }
        })
        .into_contract();
    let mut projections = contract.projections.clone();
    projections
        .canonicalize_and_validate("todo.repeated-preview")
        .unwrap();

    assert_eq!(projections.previews.len(), 2);
    assert!(matches!(
        projections.previews[0].preview.fields[0].source,
        CommandProjectionPreviewSource::Constant { .. }
    ));
    assert!(matches!(
        projections.previews[1].preview.fields[0].source,
        CommandProjectionPreviewSource::InputPath { .. }
    ));
}

fn confirmation_with_facts(facts: &[&str]) -> CommandProjectionConfirmation {
    CommandProjectionConfirmation {
        projector: "project_todos".into(),
        model: "TodoView".into(),
        key: EffectKey { fields: Vec::new() },
        partition: None,
        projector_topology: ProjectorTopologyIdentity::new(
            "project_todos",
            &facts
                .iter()
                .map(|fact| (*fact).to_string())
                .collect::<Vec<_>>(),
            &["TodoView".into()],
            &ProjectionPartitionSpec::unit(),
        ),
        protocol_topology: None,
        schema: None,
    }
}

#[test]
fn confirmation_inventory_matches_the_bounded_status_batch() {
    let maximum = crate::projection_protocol::MAX_PROJECTION_EVIDENCE_BATCH_ITEMS;
    validate_projection_confirmation_count("todo.update", maximum)
        .expect("the adapter's exact maximum must be accepted");
    let error = validate_projection_confirmation_count("todo.update", maximum + 1)
        .expect_err("one more confirmation must fail before service traffic");
    assert!(error.contains(&format!("maximum is {maximum}")), "{error}");
}

fn confirmation_with_key(
    projector: &str,
    fields: impl IntoIterator<Item = (&'static str, EffectExpression)>,
    partition: Option<EffectExpression>,
) -> CommandProjectionConfirmation {
    let fields = fields
        .into_iter()
        .map(|(field, value)| EffectFieldValue {
            field: field.into(),
            value,
        })
        .collect::<Vec<_>>();
    let columns = fields
        .iter()
        .map(|field| TableColumn {
            primary_key: true,
            ..TableColumn::new(&field.field, &field.field, ColumnType::Json)
        })
        .collect::<Vec<_>>();
    let primary_key = fields.iter().map(|field| field.field.as_str());
    let schema = Box::leak(Box::new(TableSchema {
        model_name: "TodoView".into(),
        table_name: "todo_views".into(),
        columns,
        primary_key: PrimaryKey::new(primary_key),
        version_column: None,
        foreign_keys: Vec::new(),
        indexes: Vec::new(),
        relationships: Vec::new(),
        kind: TableKind::ReadModel,
    }));
    CommandProjectionConfirmation {
        projector: projector.into(),
        model: "TodoView".into(),
        key: EffectKey { fields },
        partition,
        projector_topology: ProjectorTopologyIdentity::new(
            projector,
            &["todo.changed".into()],
            &["TodoView".into()],
            &ProjectionPartitionSpec::unit(),
        ),
        protocol_topology: Some(ProjectorTopologyId::new(1, projector, [3; 32]).unwrap()),
        schema: Some(schema),
    }
}

#[test]
fn projection_obligations_resolve_nested_canonical_wire_paths_in_declaration_order() {
    let mut contract = typed_command::<Input, Succeeded<Payload>>("todo.update").into_contract();
    contract.confirmations = vec![
        confirmation_with_key(
            "project_second",
            [
                (
                    "tenant_id",
                    EffectExpression::Input {
                        path: vec!["scope".into(), "tenantId".into()],
                    },
                ),
                (
                    "id",
                    EffectExpression::Input {
                        path: vec!["todoId".into()],
                    },
                ),
            ],
            Some(EffectExpression::Input {
                path: vec!["scope".into(), "tenantId".into()],
            }),
        ),
        confirmation_with_key(
            "project_first",
            [(
                "id",
                EffectExpression::Input {
                    path: vec!["todoId".into()],
                },
            )],
            None,
        ),
    ];
    let canonical_wire = serde_json::json!({
        "scope": { "tenantId": "tenant-7" },
        "todoId": "todo-1"
    });

    let resolved = contract
        .resolve_projection_obligations(&canonical_wire)
        .unwrap();

    assert_eq!(
        resolved
            .iter()
            .map(|obligation| obligation.projector.as_str())
            .collect::<Vec<_>>(),
        ["project_second", "project_first"]
    );
    assert_eq!(
        resolved[0]
            .key
            .fields
            .iter()
            .map(|field| field.field.as_str())
            .collect::<Vec<_>>(),
        ["tenant_id", "id"]
    );
    assert_eq!(resolved[0].key.fields[0].value, "tenant-7");
    assert_eq!(resolved[0].key.fields[1].value, "todo-1");
    assert_eq!(resolved[0].partition, Some(serde_json::json!("tenant-7")));
    assert_eq!(resolved[1].key.fields[0].value, "todo-1");
    assert_eq!(resolved[1].partition, None);
}

#[test]
fn projection_obligations_preserve_constants_and_nulls_through_serde() {
    let mut contract = typed_command::<Input, Succeeded<Payload>>("todo.update").into_contract();
    let constant = serde_json::json!({
        "nested": [1, "two", null],
        "large": u64::MAX,
    });
    contract.confirmations = vec![confirmation_with_key(
        "project_todos",
        [(
            "constant_key",
            EffectExpression::Constant {
                value: constant.clone(),
            },
        )],
        Some(EffectExpression::Null),
    )];

    let resolved = contract
        .resolve_projection_obligations(&serde_json::json!({}))
        .unwrap();

    assert_eq!(resolved[0].key.fields[0].value, constant);
    assert_eq!(resolved[0].partition, Some(serde_json::Value::Null));

    let encoded = serde_json::to_value(&resolved).unwrap();
    assert!(encoded[0]["partition"].is_null());
    let decoded: Vec<ResolvedProjectionObligation> = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded, resolved);
}

#[test]
fn projection_obligation_resolution_fails_on_absent_input_paths() {
    let mut contract = typed_command::<Input, Succeeded<Payload>>("todo.update").into_contract();
    contract.confirmations = vec![confirmation_with_key(
        "project_todos",
        [(
            "tenant_id",
            EffectExpression::Input {
                path: vec!["scope".into(), "tenantId".into()],
            },
        )],
        None,
    )];

    let error = contract
        .resolve_projection_obligations(&serde_json::json!({ "scope": null }))
        .unwrap_err();

    assert!(matches!(
        error,
        ProjectionObligationResolutionError::MissingInputPath { path, .. }
            if path == ["scope", "tenantId"]
    ));
}

#[test]
fn projection_obligation_resolution_rejects_unresolved_private_expressions() {
    let mut contract = typed_command::<Input, Succeeded<Payload>>("todo.update").into_contract();
    contract.confirmations = vec![confirmation_with_key(
        "project_todos",
        [(
            "tenant_id",
            EffectExpression::TrustedPreset {
                name: "tenant".into(),
            },
        )],
        None,
    )];
    assert!(matches!(
        contract.resolve_projection_obligations(&serde_json::json!({})),
        Err(ProjectionObligationResolutionError::TrustedPresetUnavailable {
            preset,
            ..
        }) if preset == "tenant"
    ));
    let mut session = Session::new();
    session.set("tenant", "\"tenant-7\"");
    let resolved = contract
        .resolve_projection_obligations_from_session(&serde_json::json!({}), Some(&session))
        .expect("server Session resolves the declared JSON-typed preset");
    assert_eq!(resolved[0].key.fields[0].value, "tenant-7");
    session.set("tenant", "not-json");
    assert!(matches!(
        contract
            .resolve_projection_obligations_from_session(&serde_json::json!({}), Some(&session),),
        Err(ProjectionObligationResolutionError::TrustedPresetUnavailable { .. })
    ));

    contract.confirmations = vec![confirmation_with_key(
        "project_todos",
        [],
        Some(EffectExpression::InvalidConstant {
            error: "not portable".into(),
        }),
    )];
    assert!(matches!(
        contract.resolve_projection_obligations(&serde_json::json!({})),
        Err(ProjectionObligationResolutionError::InvalidConstant {
            target,
            error,
            ..
        }) if target == "partition" && error == "not portable"
    ));
}

#[test]
fn projection_obligation_resolution_is_empty_without_confirmations() {
    let contract = typed_command::<Input, Succeeded<Payload>>("todo.check").into_contract();

    assert!(contract
        .resolve_projection_obligations(&serde_json::json!("unused"))
        .unwrap()
        .is_empty());
}

#[test]
fn finite_confirmation_requires_a_reachable_staged_outbox_fact() {
    let mut contract = typed_command::<Input, Succeeded<Payload>>("todo.create").into_contract();
    contract.confirmations = vec![confirmation_with_facts(&["todo.created", "todo.recreated"])];
    let prepared = PreparedCommand::<Succeeded<Payload>>::prepare(Payload {
        id: "todo-1".into(),
    })
    .unwrap();

    let no_fact = prepared
        .validate_commit_evidence(&contract, false, &[], &[], None)
        .unwrap_err();
    assert!(matches!(
        no_fact,
        CommandCommitProofError::UnreachableConfirmation { .. }
    ));

    let unrelated = OutboxMessage::create("message-1", "account.changed", Vec::new()).unwrap();
    assert!(matches!(
        prepared.validate_commit_evidence(&contract, false, &[unrelated], &[], None),
        Err(CommandCommitProofError::UnreachableConfirmation { .. })
    ));

    let reachable = OutboxMessage::create("message-2", "todo.created", Vec::new()).unwrap();
    prepared
        .validate_commit_evidence(&contract, false, &[reachable], &[], None)
        .unwrap();

    let directed =
        OutboxMessage::create_to("message-3", "todo.created", "todo-projector", Vec::new())
            .unwrap();
    assert!(matches!(
        prepared.validate_commit_evidence(&contract, false, &[directed], &[], None),
        Err(CommandCommitProofError::UnreachableConfirmation { .. })
    ));

    let mut published = OutboxMessage::create("message-4", "todo.created", Vec::new()).unwrap();
    published.status = crate::outbox::OutboxMessageStatus::Published;
    assert!(matches!(
        prepared.validate_commit_evidence(&contract, false, &[published], &[], None),
        Err(CommandCommitProofError::UnreachableConfirmation { .. })
    ));

    let mut failed = OutboxMessage::create("message-5", "todo.created", Vec::new()).unwrap();
    failed.status = crate::outbox::OutboxMessageStatus::Failed;
    assert!(matches!(
        prepared.validate_commit_evidence(&contract, false, &[failed], &[], None),
        Err(CommandCommitProofError::UnreachableConfirmation { .. })
    ));
}

#[test]
fn succeeded_without_confirmations_allows_an_empty_domain_batch() {
    let contract = typed_command::<Input, Succeeded<Payload>>("todo.check").into_contract();
    let prepared = PreparedCommand::<Succeeded<Payload>>::prepare(Payload {
        id: "todo-1".into(),
    })
    .unwrap();

    prepared
        .validate_commit_evidence(&contract, false, &[], &[], None)
        .unwrap();
}

#[test]
fn causal_without_a_finite_confirmation_fails_at_commit_validation() {
    let contract = typed_command::<Input, Eventual<Payload>>("todo.create").into_contract();
    let prepared = PreparedCommand::<Eventual<Payload>>::prepare(Payload {
        id: "todo-1".into(),
    })
    .unwrap();
    let fact = OutboxMessage::create("message-1", "todo.created", Vec::new()).unwrap();

    assert_eq!(
        prepared
            .validate_commit_evidence(&contract, false, &[fact], &[], None)
            .unwrap_err(),
        CommandCommitProofError::CausalHasNoConfirmations
    );
}

#[test]
fn per_route_fingerprint_is_stable_and_contract_sensitive() {
    let first = typed_command::<Input, Succeeded<Payload>>("todo.create")
        .roles(["writer", "admin"])
        .into_contract();
    let reordered = typed_command::<Input, Succeeded<Payload>>("todo.create")
        .roles(["admin", "writer"])
        .into_contract();
    let renamed = typed_command::<Input, Succeeded<Payload>>("todo.rename")
        .roles(["admin", "writer"])
        .into_contract();

    assert_eq!(first.fingerprint_bytes(), reordered.fingerprint_bytes());
    assert_ne!(first.fingerprint_bytes(), renamed.fingerprint_bytes());
}

#[test]
fn command_fingerprints_canonicalize_nested_json_object_keys() {
    fn contract_with_constant(order: [&str; 2]) -> TypedCommandContract {
        let mut value = serde_json::Map::new();
        for key in order {
            value.insert(
                key.into(),
                serde_json::json!(if key == "a" { 1 } else { 2 }),
            );
        }
        let mut contract =
            typed_command::<Input, Succeeded<Payload>>("todo.update").into_contract();
        contract.effects = CommandEffects::new([CommandEffect::Patch {
            model: "Todo".into(),
            key: EffectKey {
                fields: vec![EffectFieldValue {
                    field: "id".into(),
                    value: EffectExpression::Input {
                        path: vec!["id".into()],
                    },
                }],
            },
            fields: vec![EffectFieldValue {
                field: "metadata".into(),
                value: EffectExpression::Constant {
                    value: serde_json::Value::Object(value),
                },
            }],
        }]);
        contract
    }

    let reverse_insertion = contract_with_constant(["z", "a"]);
    let sorted_insertion = contract_with_constant(["a", "z"]);
    assert_eq!(
        reverse_insertion.fingerprint_bytes(),
        sorted_insertion.fingerprint_bytes()
    );

    let route_fingerprint = format!(
        "sha256:{}",
        reverse_insertion
            .fingerprint_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    let reverse_binding =
        TypedServiceCommandBinding::from_contracts("todos", &[reverse_insertion]).unwrap();
    let sorted_binding =
        TypedServiceCommandBinding::from_contracts("todos", &[sorted_insertion]).unwrap();
    assert_eq!(
        reverse_binding.structural_fingerprint,
        sorted_binding.structural_fingerprint
    );
    assert_eq!(
        (
            route_fingerprint.as_str(),
            reverse_binding.structural_fingerprint.as_str(),
        ),
        (
            "sha256:db980e553f62b65ee01600c23c0331e97b5083f5e9f3b65498cb4cde344a0204",
            "sha256:ba291c9b13aafb68a135b5b8112e3e2ebf1fa1e23819eb210219c60b186c265f",
        )
    );
}

#[test]
fn binding_rejects_missing_graphql_type_ids() {
    let mut contract = typed_command::<Input, Succeeded<Payload>>("todo.create").into_contract();
    contract.input.type_id = None;
    let error = TypedServiceCommandBinding::from_contracts("todos", &[contract]).unwrap_err();
    assert!(error.contains("input GraphQL metadata is missing"));
}

#[test]
fn binding_canonicalizes_fields_and_roles_but_preserves_effect_order() {
    let mut first = typed_command::<Input, Succeeded<Payload>>("todo.create")
        .roles(["writer", "admin"])
        .into_contract();
    first.input.fields.push(GraphqlTypeField {
        name: "z_extra".into(),
        type_name: "String".into(),
        nullable: true,
        list: false,
        item_nullable: false,
        nested: None,
    });
    first.effects.operations = vec![
        CommandEffect::InvalidateModel {
            model: "Zed".into(),
        },
        CommandEffect::InvalidateModel {
            model: "Alpha".into(),
        },
    ];

    let mut second = first.clone();
    second.roles.reverse();
    second.input.fields.reverse();
    let first = TypedServiceCommandBinding::from_contracts("todos", &[first]).unwrap();
    let second = TypedServiceCommandBinding::from_contracts("todos", &[second]).unwrap();
    assert_eq!(first, second);

    let mut reordered = typed_command::<Input, Succeeded<Payload>>("todo.create")
        .roles(["writer", "admin"])
        .into_contract();
    reordered.input.fields.push(GraphqlTypeField {
        name: "z_extra".into(),
        type_name: "String".into(),
        nullable: true,
        list: false,
        item_nullable: false,
        nested: None,
    });
    reordered.effects.operations = vec![
        CommandEffect::InvalidateModel {
            model: "Alpha".into(),
        },
        CommandEffect::InvalidateModel {
            model: "Zed".into(),
        },
    ];
    let reordered = TypedServiceCommandBinding::from_contracts("todos", &[reordered]).unwrap();
    assert_ne!(
        first.structural_fingerprint,
        reordered.structural_fingerprint
    );
}
