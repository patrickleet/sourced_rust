use std::any::TypeId;

use super::*;
use crate::graphql::command_contract::{CommandEffects, TypedCommandContract};
use crate::graphql::commands::TypedCommandInventory;
use crate::graphql::{GraphqlTypeDef, GraphqlTypeField};
use crate::table::{ColumnType, PrimaryKey, RelationshipDef, TableColumn, TableKind};

fn orders() -> TableSchema {
    TableSchema {
        model_name: "OrderView".into(),
        table_name: "orders".into(),
        columns: vec![
            TableColumn {
                primary_key: true,
                ..TableColumn::new("order_id", "order_id", ColumnType::Text)
            },
            TableColumn::new("customer_id", "customer_id", ColumnType::Text),
            TableColumn::new("status", "status", ColumnType::Text),
            TableColumn {
                jsonb: true,
                ..TableColumn::new("meta", "meta", ColumnType::Json)
            },
        ],
        primary_key: PrimaryKey::new(["order_id"]),
        version_column: Some("_sourced_version".into()),
        foreign_keys: Vec::new(),
        indexes: Vec::new(),
        relationships: Vec::new(),
        kind: TableKind::ReadModel,
    }
}

fn operational() -> TableSchema {
    TableSchema {
        model_name: "Outbox".into(),
        table_name: "outbox".into(),
        columns: vec![TableColumn {
            primary_key: true,
            ..TableColumn::new("id", "id", ColumnType::Text)
        }],
        primary_key: PrimaryKey::new(["id"]),
        version_column: None,
        foreign_keys: Vec::new(),
        indexes: Vec::new(),
        relationships: Vec::new(),
        kind: TableKind::Operational,
    }
}

fn test_command(
    command_name: &str,
    field_name: &str,
    output: GraphqlTypeDef,
) -> TypedCommandContract {
    let input_type_id = TypeId::of::<String>();
    let output_type_id = TypeId::of::<()>();
    TypedCommandContract {
        name: command_name.into(),
        field_name: field_name.into(),
        roles: Vec::new(),
        input: GraphqlTypeDef::new(
            "TestCommandInput",
            vec![GraphqlTypeField {
                name: "id".into(),
                type_name: "String".into(),
                nullable: false,
                list: false,
                item_nullable: false,
                nested: None,
            }],
        )
        .with_type_id(input_type_id),
        output: output.with_type_id(output_type_id),
        input_type_id,
        output_type_id,
        consistency: CommandConsistency::Succeeded,
        input_defaults: Vec::new(),
        effects: CommandEffects::revalidate(),
        confirmations: Vec::new(),
        projected_model: None,
        direct_projection: None,
        projections: Default::default(),
    }
}

fn test_inventory(
    contracts: impl IntoIterator<Item = TypedCommandContract>,
) -> TypedCommandInventory {
    TypedCommandInventory::from_contracts(&contracts.into_iter().collect::<Vec<_>>()).unwrap()
}

#[test]
fn build_surface_skips_operational_and_lists_roots() {
    let surface =
        build_surface(&[orders(), operational()], &SurfaceOptions::sqlite()).expect("surface");
    assert!(surface.models.contains_key("OrderView"));
    assert!(!surface.models.contains_key("Outbox"));
    let roots = surface.query_root_names();
    assert!(roots.contains(&"orders"));
    assert!(roots.contains(&"orders_by_pk"));
    assert!(roots.contains(&"orders_aggregate"));
}

#[test]
fn sqlite_surface_omits_pg_json_comparison_ops() {
    let surface = build_surface(&[orders()], &SurfaceOptions::sqlite()).unwrap();
    let ops = surface.comparison_ops_for_scalar("JSON");
    assert!(ops.contains(&"_eq"));
    for forbidden in ["_contains", "_contained_in", "_has_key"] {
        assert!(
            !ops.contains(&forbidden),
            "SQLite must not expose {forbidden}"
        );
    }
}

#[test]
fn postgres_surface_includes_pg_json_comparison_ops() {
    let surface = build_surface(&[orders()], &SurfaceOptions::postgres()).unwrap();
    let ops = surface.comparison_ops_for_scalar("JSON");
    for required in ["_contains", "_contained_in", "_has_key"] {
        assert!(ops.contains(&required), "Postgres missing {required}");
    }
}

#[test]
fn surface_rejects_duplicate_stable_model_ids() {
    let error = build_surface(&[orders(), orders()], &SurfaceOptions::sqlite()).unwrap_err();
    assert!(
        error.contains("duplicate table model id `OrderView`"),
        "{error}"
    );
}

#[test]
fn surface_rejects_model_and_generated_auxiliary_type_collision() {
    let mut colliding = orders();
    colliding.model_name = "orders_bool_exp".into();
    colliding.table_name = "other_orders".into();
    let error = build_surface(&[orders(), colliding], &SurfaceOptions::sqlite()).unwrap_err();
    assert!(
        error.contains("`orders_bool_exp` collides with another Surface type"),
        "{error}"
    );
}

#[test]
fn projector_topology_rejects_duplicate_and_empty_ids() {
    let surface = build_surface(&[orders()], &SurfaceOptions::sqlite()).unwrap();
    let duplicate = surface
        .clone()
        .with_projectors([
            SurfaceProjector::new("orders")
                .facts(["order.changed"])
                .models(["OrderView"]),
            SurfaceProjector::new("orders")
                .facts(["order.created"])
                .models(["OrderView"]),
        ])
        .unwrap_err();
    assert!(duplicate.contains("duplicate projector name `orders`"));

    let empty_fact = surface
        .clone()
        .with_projectors([SurfaceProjector::new("orders")
            .facts([""])
            .models(["OrderView"])])
        .unwrap_err();
    assert!(empty_fact.contains("fact id must not be empty"));

    let empty_model = surface
        .clone()
        .with_projectors([SurfaceProjector::new("orders")
            .facts(["order.changed"])
            .models([""])])
        .unwrap_err();
    assert!(empty_model.contains("model id must not be empty"));

    let no_facts = surface
        .clone()
        .with_projectors([SurfaceProjector::new("orders").models(["OrderView"])])
        .unwrap_err();
    assert!(no_facts.contains("must declare at least one fact"));
    let no_models = surface
        .with_projectors([SurfaceProjector::new("orders").facts(["order.changed"])])
        .unwrap_err();
    assert!(no_models.contains("must declare at least one model"));
}

#[test]
fn direct_projection_owner_requires_models_but_not_facts() {
    let surface = build_surface(&[orders()], &SurfaceOptions::sqlite())
        .unwrap()
        .with_projection_owners([SurfaceDirectProjection::new("orders")
            .models(["OrderView"])
            .change_epoch("orders-v1")
            .into()])
        .unwrap();
    let [owner] = surface.projection_owners() else {
        panic!("one direct owner should be retained");
    };
    assert!(owner.is_direct());
    assert!(owner.facts.is_empty());
    assert_eq!(owner.models, vec!["OrderView".to_string()]);

    let error = build_surface(&[orders()], &SurfaceOptions::sqlite())
        .unwrap()
        .with_projection_owners([SurfaceDirectProjection::new("orders")
            .change_epoch("orders-v1")
            .into()])
        .unwrap_err();
    assert!(error.contains("must declare at least one model"));
}

#[test]
fn selected_surfaces_reject_command_and_projector_reattachment() {
    let full = build_surface(&[orders()], &SurfaceOptions::sqlite()).unwrap();
    let grants = BTreeMap::from([("OrderView".into(), RoleGrant::all_columns())]);
    let selected = surface_for_role(&full, "user", &grants).unwrap();
    assert!(selected
        .clone()
        .with_typed_commands(&TypedCommandInventory::empty())
        .unwrap_err()
        .contains("before authorization selection"));
    assert!(selected
        .with_projectors(Vec::<SurfaceProjector>::new())
        .unwrap_err()
        .contains("before authorization selection"));

    let grants_by_role = BTreeMap::from([("user".into(), grants)]);
    let application =
        surface_for_application(&full, "web", &["user".into()], &grants_by_role).unwrap();
    assert!(application
        .clone()
        .with_typed_commands(&TypedCommandInventory::empty())
        .unwrap_err()
        .contains("before authorization selection"));
    assert!(application
        .with_projectors(Vec::<SurfaceProjector>::new())
        .unwrap_err()
        .contains("before authorization selection"));
}

#[test]
fn role_policy_rejects_non_finite_and_hides_js_unsafe_integers() {
    let full = build_surface(&[orders()], &SurfaceOptions::sqlite()).unwrap();
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let grants = BTreeMap::from([(
            "OrderView".into(),
            RoleGrant::all_columns().rows(super::super::col("status").eq(value)),
        )]);
        assert!(surface_for_role(&full, "user", &grants)
            .unwrap_err()
            .contains("must be finite"));
    }

    let mut integer_orders = orders();
    integer_orders.columns.push(TableColumn::new(
        "sequence",
        "sequence",
        ColumnType::Integer,
    ));
    let full = build_surface(&[integer_orders], &SurfaceOptions::sqlite()).unwrap();
    let grants = BTreeMap::from([(
        "OrderView".into(),
        RoleGrant::all_columns().rows(super::super::col("sequence").eq(9_007_199_254_740_992_i64)),
    )]);
    let selected = surface_for_role(&full, "user", &grants).unwrap();
    assert_eq!(
        selected.models["OrderView"].row_policy,
        SurfaceRowPolicy::ServerOnly
    );
}

#[test]
fn command_surface_rejects_duplicate_mutation_field_ids() {
    let output = GraphqlTypeDef::new(
        "TestCommandPayload",
        vec![GraphqlTypeField {
            name: "id".into(),
            type_name: "String".into(),
            nullable: false,
            list: false,
            item_nullable: false,
            nested: None,
        }],
    );
    let commands = test_inventory([
        test_command("order.create", "orders_write", output.clone()),
        test_command("order.replace", "orders_write", output),
    ]);
    let error = build_surface(&[orders()], &SurfaceOptions::sqlite())
        .unwrap()
        .with_typed_commands(&commands)
        .unwrap_err();
    assert!(error.contains("duplicate command mutation field `orders_write`"));
}

#[test]
fn command_surface_rejects_empty_nested_and_surface_colliding_types() {
    let empty = test_inventory([test_command(
        "order.empty",
        "order_empty",
        GraphqlTypeDef::new("EmptyPayload", Vec::new()),
    )]);
    let error = build_surface(&[orders()], &SurfaceOptions::sqlite())
        .unwrap()
        .with_typed_commands(&empty)
        .unwrap_err();
    assert!(error.contains("must declare at least one field"), "{error}");

    let nested = test_inventory([test_command(
        "order.nested_empty",
        "order_nested_empty",
        GraphqlTypeDef::new(
            "OuterPayload",
            vec![GraphqlTypeField {
                name: "inner".into(),
                type_name: "InnerPayload".into(),
                nullable: false,
                list: false,
                item_nullable: false,
                nested: Some(Box::new(GraphqlTypeDef::new("InnerPayload", Vec::new()))),
            }],
        ),
    )]);
    let error = build_surface(&[orders()], &SurfaceOptions::sqlite())
        .unwrap()
        .with_typed_commands(&nested)
        .unwrap_err();
    assert!(error.contains("`InnerPayload` must declare at least one field"));

    let collision = test_inventory([test_command(
        "order.collision",
        "order_collision",
        GraphqlTypeDef::new(
            "OrderView",
            vec![GraphqlTypeField {
                name: "order_id".into(),
                type_name: "String".into(),
                nullable: false,
                list: false,
                item_nullable: false,
                nested: None,
            }],
        ),
    )]);
    let error = build_surface(&[orders()], &SurfaceOptions::sqlite())
        .unwrap()
        .with_typed_commands(&collision)
        .unwrap_err();
    assert!(error.contains("collides with a Surface GraphQL type"));
}

#[test]
fn projected_output_reuse_and_sdl_emission_use_the_same_exact_predicate() {
    let schema: &'static TableSchema = Box::leak(Box::new(orders()));
    let projected_model = CommandProjectedModel {
        output_type_id: std::any::TypeId::of::<()>(),
        model: "OrderView".into(),
        table: "orders".into(),
        schema,
        partition: None,
    };
    let projected_command = |output: SurfaceTypeDef| SurfaceCommand {
        command_name: "order.projected".into(),
        field_name: "order_projected".into(),
        roles: Vec::new(),
        input: SurfaceCommandShape::None,
        output: SurfaceCommandShape::Typed(output),
        consistency: CommandConsistency::Projected,
        input_defaults: Vec::new(),
        effects: Some(CommandEffects::revalidate()),
        confirmations: Vec::new(),
        projected_model: Some(projected_model.clone()),
        direct_projection: None,
        projections: Default::default(),
        confirmation_unavailable: false,
    };
    let one_string_field = |name: &str| SurfaceTypeDef {
        name: name.into(),
        fields: vec![SurfaceTypeField {
            name: "order_id".into(),
            type_name: "String".into(),
            nullable: false,
            list: false,
            item_nullable: false,
            nested: None,
        }],
    };

    let mut custom = build_surface(&[orders()], &SurfaceOptions::sqlite()).unwrap();
    custom.commands = vec![projected_command(one_string_field(
        "CustomProjectedPayload",
    ))];
    validate_and_canonicalize_commands(
        &custom.models,
        &custom.comparison_ops,
        &mut custom.commands,
    )
    .unwrap();
    let sdl = crate::graphql::sdl::graphql_sdl_from_surface(&custom).unwrap();
    assert!(
        sdl.contains("type CustomProjectedPayload {"),
        "a non-reused projected output must still be emitted: {sdl}"
    );

    let mut mismatched = build_surface(&[orders()], &SurfaceOptions::sqlite()).unwrap();
    mismatched.commands = vec![projected_command(one_string_field("OrderView"))];
    let error = validate_and_canonicalize_commands(
        &mismatched.models,
        &mismatched.comparison_ops,
        &mut mismatched.commands,
    )
    .unwrap_err();
    assert!(
        error.contains("does not match the normalized Surface model columns"),
        "{error}"
    );
}

#[test]
fn surface_for_role_drops_ungranted_columns_and_models() {
    let full = build_surface(&[orders()], &SurfaceOptions::sqlite()).unwrap();
    let mut grants = BTreeMap::new();
    grants.insert(
        "OrderView".to_string(),
        RoleGrant::columns(["order_id", "status"]),
    );
    let role_surface = surface_for_role(&full, "user", &grants).unwrap();
    let model = role_surface.models.get("OrderView").expect("granted");
    let col_names: Vec<_> = model.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(col_names, vec!["order_id", "status"]);
    assert!(!col_names.contains(&"customer_id"));
    assert!(!col_names.contains(&"meta"));

    let empty = surface_for_role(&full, "anon", &BTreeMap::new()).unwrap();
    assert!(empty.models.is_empty());
    assert!(empty.query_fields.is_empty());
}

#[test]
fn role_surface_legacy_effects_never_become_v2_client_authority() {
    let mut full = build_surface(&[orders()], &SurfaceOptions::sqlite()).unwrap();
    let input = SurfaceTypeDef {
        name: "UpdateOrderInput".into(),
        fields: vec![
            SurfaceTypeField {
                name: "order_id".into(),
                type_name: "String".into(),
                nullable: false,
                list: false,
                item_nullable: false,
                nested: None,
            },
            SurfaceTypeField {
                name: "customer_id".into(),
                type_name: "String".into(),
                nullable: false,
                list: false,
                item_nullable: false,
                nested: None,
            },
        ],
    };
    let key = EffectKey {
        fields: vec![EffectFieldValue {
            field: "order_id".into(),
            value: EffectExpression::Input {
                path: vec!["order_id".into()],
            },
        }],
    };
    let denied_field_command = SurfaceCommand {
        command_name: "order.assign_customer".into(),
        field_name: "order_assign_customer".into(),
        roles: Vec::new(),
        input: SurfaceCommandShape::Typed(input.clone()),
        output: SurfaceCommandShape::Typed(SurfaceTypeDef {
            name: "AssignCustomerPayload".into(),
            fields: vec![SurfaceTypeField {
                name: "order_id".into(),
                type_name: "String".into(),
                nullable: false,
                list: false,
                item_nullable: false,
                nested: None,
            }],
        }),
        consistency: CommandConsistency::Succeeded,
        input_defaults: Vec::new(),
        effects: Some(CommandEffects::new([CommandEffect::Patch {
            model: "OrderView".into(),
            key: key.clone(),
            fields: vec![EffectFieldValue {
                field: "customer_id".into(),
                value: EffectExpression::Input {
                    path: vec!["customer_id".into()],
                },
            }],
        }])),
        confirmations: Vec::new(),
        projected_model: None,
        direct_projection: None,
        projections: Default::default(),
        confirmation_unavailable: false,
    };
    let trusted_preset_command = SurfaceCommand {
        command_name: "order.apply_preset".into(),
        field_name: "order_apply_preset".into(),
        roles: Vec::new(),
        input: SurfaceCommandShape::Typed(input),
        output: SurfaceCommandShape::Typed(SurfaceTypeDef {
            name: "ApplyPresetPayload".into(),
            fields: vec![SurfaceTypeField {
                name: "order_id".into(),
                type_name: "String".into(),
                nullable: false,
                list: false,
                item_nullable: false,
                nested: None,
            }],
        }),
        consistency: CommandConsistency::Succeeded,
        input_defaults: Vec::new(),
        effects: Some(CommandEffects::new([CommandEffect::Patch {
            model: "OrderView".into(),
            key,
            fields: vec![EffectFieldValue {
                field: "status".into(),
                value: EffectExpression::TrustedPreset {
                    name: "tenant-secret".into(),
                },
            }],
        }])),
        confirmations: Vec::new(),
        projected_model: None,
        direct_projection: None,
        projections: Default::default(),
        confirmation_unavailable: false,
    };
    full.commands = vec![denied_field_command, trusted_preset_command];
    validate_and_canonicalize_commands(&full.models, &full.comparison_ops, &mut full.commands)
        .unwrap();

    let selected = surface_for_role(
        &full,
        "user",
        &BTreeMap::from([(
            "OrderView".into(),
            RoleGrant::columns(["order_id", "status"]),
        )]),
    )
    .unwrap();
    let denied = selected
        .commands
        .iter()
        .find(|command| command.command_name == "order.assign_customer")
        .expect("denied-field command");
    assert!(denied
        .effects
        .as_ref()
        .is_some_and(|effects| effects.operations.is_empty()));
    let trusted = selected
        .commands
        .iter()
        .find(|command| command.command_name == "order.apply_preset")
        .expect("trusted-preset command");
    assert!(trusted
        .effects
        .as_ref()
        .is_some_and(|effects| !effects.operations.is_empty()));

    let manifest = super::super::client_manifest_from_surface(
        "orders",
        super::super::ClientSurfaceIdentity::role("user"),
        &selected,
    )
    .unwrap();
    let denied = manifest
        .commands
        .iter()
        .find(|command| command.name == "order.assign_customer")
        .expect("denied-field manifest command");
    assert!(denied.extensions.effects.is_none());
    assert!(denied.extensions.trusted_presets.is_empty());
    let denied_effects_json = serde_json::to_string(&denied.extensions.effects).unwrap();
    assert!(
        !denied_effects_json.contains("customer_id"),
        "{denied_effects_json}"
    );
    assert!(
        !denied_effects_json.contains("tenant-secret"),
        "{denied_effects_json}"
    );

    let trusted = manifest
        .commands
        .iter()
        .find(|command| command.name == "order.apply_preset")
        .expect("trusted-preset manifest command");
    assert!(trusted.extensions.effects.is_none());
    assert!(trusted.extensions.trusted_presets.is_empty());
    let trusted_json = serde_json::to_string(trusted).unwrap();
    assert!(!trusted_json.contains("tenant-secret"), "{trusted_json}");
    let preset_descriptors_json =
        serde_json::to_string(&trusted.extensions.trusted_presets).unwrap();
    assert!(
        !preset_descriptors_json.contains("\"value\":"),
        "{preset_descriptors_json}"
    );
}

#[test]
fn pool_free_role_selection_rejects_invalid_grants_and_policy_references() {
    let full = build_surface(&[orders()], &SurfaceOptions::sqlite()).unwrap();

    let unknown_model = surface_for_role(
        &full,
        "user",
        &BTreeMap::from([("TypoView".into(), RoleGrant::all_columns())]),
    )
    .unwrap_err();
    assert!(unknown_model.contains("unknown model `TypoView`"));

    let unknown_column = surface_for_role(
        &full,
        "user",
        &BTreeMap::from([(
            "OrderView".into(),
            RoleGrant::columns(["order_id", "statuz"]),
        )]),
    )
    .unwrap_err();
    assert!(unknown_column.contains("unknown column `statuz` in permission"));

    let unknown_filter_column = surface_for_role(
        &full,
        "user",
        &BTreeMap::from([(
            "OrderView".into(),
            RoleGrant::all_columns().rows(super::super::col("statuz").eq("open")),
        )]),
    )
    .unwrap_err();
    assert!(unknown_filter_column.contains("unknown column `statuz` in filter"));

    let unknown_relationship = surface_for_role(
        &full,
        "user",
        &BTreeMap::from([(
            "OrderView".into(),
            RoleGrant::all_columns().rows(super::super::rel(
                "customer",
                super::super::col("id").eq("c1"),
            )),
        )]),
    )
    .unwrap_err();
    assert!(unknown_relationship.contains("is not a relationship on model `OrderView`"));
}

#[test]
fn pool_free_role_selection_rejects_mistyped_row_policy_literals() {
    let full = build_surface(&[orders()], &SurfaceOptions::sqlite()).unwrap();
    let cmp_error = surface_for_role(
        &full,
        "user",
        &BTreeMap::from([(
            "OrderView".into(),
            RoleGrant::all_columns().rows(FilterExpr::Cmp {
                column: "status".into(),
                op: super::super::filter::CmpOp::Eq,
                rhs: Operand::Lit(super::super::LitValue::Json(serde_json::json!("open"))),
            }),
        )]),
    )
    .unwrap_err();
    assert!(cmp_error.contains("literal kind `json`"), "{cmp_error}");
    assert!(cmp_error.contains("column `status`"), "{cmp_error}");

    let in_error = surface_for_role(
        &full,
        "user",
        &BTreeMap::from([(
            "OrderView".into(),
            RoleGrant::all_columns().rows(FilterExpr::In {
                column: "status".into(),
                values: vec![
                    Operand::from("open"),
                    Operand::Lit(super::super::LitValue::Json(serde_json::json!("closed"))),
                ],
                negated: false,
            }),
        )]),
    )
    .unwrap_err();
    assert!(in_error.contains("IN operand 1"), "{in_error}");
}

#[test]
fn selected_surface_debug_does_not_leak_denied_schema_metadata() {
    let full = build_surface(&[orders()], &SurfaceOptions::sqlite()).unwrap();
    let selected = surface_for_role(
        &full,
        "user",
        &BTreeMap::from([(
            "OrderView".into(),
            RoleGrant::columns(["order_id", "status"]),
        )]),
    )
    .unwrap();

    let debug = format!("{selected:?}");
    assert!(debug.contains("OrderView"));
    assert!(!debug.contains("customer_id"), "{debug}");
    assert!(!debug.contains("meta"), "{debug}");
}

#[test]
fn surface_for_role_omits_aggregate_without_grant() {
    let full = build_surface(&[orders()], &SurfaceOptions::sqlite()).unwrap();
    let mut grants = BTreeMap::new();
    grants.insert("OrderView".to_string(), RoleGrant::all_columns());
    let role_surface = surface_for_role(&full, "user", &grants).unwrap();
    let names = role_surface.query_root_names();
    assert!(names.contains(&"orders"));
    assert!(!names.contains(&"orders_aggregate"));

    let mut admin = BTreeMap::new();
    admin.insert(
        "OrderView".to_string(),
        RoleGrant::all_columns().with_aggregations(),
    );
    let admin_surface = surface_for_role(&full, "admin", &admin).unwrap();
    assert!(admin_surface
        .query_root_names()
        .contains(&"orders_aggregate"));
}

#[test]
fn relationship_only_when_target_on_surface() {
    let parent = TableSchema {
        model_name: "ParentView".into(),
        table_name: "parents".into(),
        columns: vec![TableColumn {
            primary_key: true,
            ..TableColumn::new("id", "id", ColumnType::Text)
        }],
        primary_key: PrimaryKey::new(["id"]),
        version_column: None,
        foreign_keys: Vec::new(),
        indexes: Vec::new(),
        relationships: vec![RelationshipDef {
            field_name: "children".into(),
            kind: RelationshipKind::HasMany,
            target_model: "ChildView".into(),
            foreign_key: Some("parent_id".into()),
            through: None,
            target_foreign_key: None,
        }],
        kind: TableKind::ReadModel,
    };
    let child = TableSchema {
        model_name: "ChildView".into(),
        table_name: "children".into(),
        columns: vec![
            TableColumn {
                primary_key: true,
                ..TableColumn::new("id", "id", ColumnType::Text)
            },
            TableColumn::new("parent_id", "parent_id", ColumnType::Text),
        ],
        primary_key: PrimaryKey::new(["id"]),
        version_column: None,
        foreign_keys: Vec::new(),
        indexes: Vec::new(),
        relationships: Vec::new(),
        kind: TableKind::ReadModel,
    };
    let both = build_surface(&[parent.clone(), child], &SurfaceOptions::sqlite()).unwrap();
    assert!(both
        .models
        .get("ParentView")
        .unwrap()
        .relationships
        .iter()
        .any(|r| r.name == "children"));

    let parent_only = build_surface(&[parent], &SurfaceOptions::sqlite()).unwrap();
    assert!(parent_only
        .models
        .get("ParentView")
        .unwrap()
        .relationships
        .is_empty());
}

#[test]
fn surface_rejects_relationship_and_generated_aggregate_field_collisions() {
    let child = TableSchema {
        model_name: "CollisionChild".into(),
        table_name: "collision_children".into(),
        columns: vec![
            TableColumn {
                primary_key: true,
                ..TableColumn::new("id", "id", ColumnType::Text)
            },
            TableColumn::new("parent_id", "parent_id", ColumnType::Text),
        ],
        primary_key: PrimaryKey::new(["id"]),
        version_column: None,
        foreign_keys: Vec::new(),
        indexes: Vec::new(),
        relationships: Vec::new(),
        kind: TableKind::ReadModel,
    };
    let parent = TableSchema {
        model_name: "CollisionParent".into(),
        table_name: "collision_parents".into(),
        columns: vec![
            TableColumn {
                primary_key: true,
                ..TableColumn::new("id", "id", ColumnType::Text)
            },
            TableColumn::new("children_aggregate", "children_aggregate", ColumnType::Text),
        ],
        primary_key: PrimaryKey::new(["id"]),
        version_column: None,
        foreign_keys: Vec::new(),
        indexes: Vec::new(),
        relationships: vec![RelationshipDef {
            field_name: "children".into(),
            kind: RelationshipKind::HasMany,
            target_model: "CollisionChild".into(),
            foreign_key: Some("parent_id".into()),
            through: None,
            target_foreign_key: None,
        }],
        kind: TableKind::ReadModel,
    };
    let error = build_surface(&[parent, child], &SurfaceOptions::sqlite()).unwrap_err();
    assert!(error.contains("relationship aggregate `children_aggregate` collides"));
}

#[test]
fn relationship_keys_canonicalize_rust_field_names_to_graphql_columns() {
    let account = TableSchema {
        model_name: "AccountView".into(),
        table_name: "accounts".into(),
        columns: vec![TableColumn {
            primary_key: true,
            ..TableColumn::new("account_id", "account_id", ColumnType::Text)
        }],
        primary_key: PrimaryKey::new(["account_id"]),
        version_column: None,
        foreign_keys: Vec::new(),
        indexes: Vec::new(),
        relationships: Vec::new(),
        kind: TableKind::ReadModel,
    };
    let order = TableSchema {
        model_name: "RenamedOrderView".into(),
        table_name: "renamed_orders".into(),
        columns: vec![
            TableColumn {
                primary_key: true,
                ..TableColumn::new("order_id", "order_id", ColumnType::Text)
            },
            TableColumn::new("accountId", "account_id", ColumnType::Text),
        ],
        primary_key: PrimaryKey::new(["order_id"]),
        version_column: None,
        foreign_keys: Vec::new(),
        indexes: Vec::new(),
        relationships: vec![RelationshipDef {
            field_name: "account".into(),
            kind: RelationshipKind::BelongsTo,
            target_model: "AccountView".into(),
            foreign_key: Some("accountId".into()),
            through: None,
            target_foreign_key: None,
        }],
        kind: TableKind::ReadModel,
    };
    let surface = build_surface(&[order, account], &SurfaceOptions::sqlite()).unwrap();
    let relationship = &surface.models["RenamedOrderView"].relationships[0];
    assert_eq!(
        relationship.keys,
        SurfaceRelationshipKeys::Direct {
            local: vec!["account_id".into()],
            remote: vec!["account_id".into()],
        }
    );
}

#[test]
fn pool_free_role_selection_rejects_only_reachable_composite_relationships() {
    let composite = TableSchema {
        model_name: "CompositeView".into(),
        table_name: "composites".into(),
        columns: vec![
            TableColumn {
                primary_key: true,
                ..TableColumn::new("tenant_id", "tenant_id", ColumnType::Text)
            },
            TableColumn {
                primary_key: true,
                ..TableColumn::new("record_id", "record_id", ColumnType::Text)
            },
        ],
        primary_key: PrimaryKey::new(["tenant_id", "record_id"]),
        version_column: None,
        foreign_keys: Vec::new(),
        indexes: Vec::new(),
        relationships: Vec::new(),
        kind: TableKind::ReadModel,
    };
    let simple = TableSchema {
        model_name: "SimpleView".into(),
        table_name: "simples".into(),
        columns: vec![
            TableColumn {
                primary_key: true,
                ..TableColumn::new("simple_id", "simple_id", ColumnType::Text)
            },
            TableColumn::new("tenant_id", "tenant_id", ColumnType::Text),
        ],
        primary_key: PrimaryKey::new(["simple_id"]),
        version_column: None,
        foreign_keys: Vec::new(),
        indexes: Vec::new(),
        relationships: vec![RelationshipDef {
            field_name: "composite".into(),
            kind: RelationshipKind::BelongsTo,
            target_model: "CompositeView".into(),
            foreign_key: Some("tenant_id".into()),
            through: None,
            target_foreign_key: None,
        }],
        kind: TableKind::ReadModel,
    };
    let full = build_surface(&[simple, composite], &SurfaceOptions::sqlite()).unwrap();

    // Hidden catalog metadata cannot create a false rejection.
    let source_only = surface_for_role(
        &full,
        "source-only",
        &BTreeMap::from([("SimpleView".into(), RoleGrant::all_columns())]),
    )
    .unwrap();
    assert!(source_only.models["SimpleView"].relationships.is_empty());

    // A server-only policy may legitimately traverse a denied model, but
    // it must still be rejected when the runtime's current join compiler
    // cannot represent that composite identity safely.
    let hidden_policy_error = surface_for_role(
        &full,
        "source-policy",
        &BTreeMap::from([(
            "SimpleView".into(),
            RoleGrant::all_columns().rows(super::super::rel(
                "composite",
                super::super::col("tenant_id").eq("tenant-a"),
            )),
        )]),
    )
    .unwrap_err();
    assert!(
        hidden_policy_error.contains("composite-key topology"),
        "{hidden_policy_error}"
    );

    // Once both models are reachable, pool-free export fails at the same
    // selected-Surface boundary as runtime engine construction.
    let error = surface_for_role(
        &full,
        "both",
        &BTreeMap::from([
            ("CompositeView".into(), RoleGrant::all_columns()),
            ("SimpleView".into(), RoleGrant::all_columns()),
        ]),
    )
    .unwrap_err();
    assert!(error.contains("relationship topology"), "{error}");
}

/// Production path: build_surface → surface_for_role → SDL (gap A10).
#[test]
fn role_sdl_production_path_omits_ungranted_columns() {
    use super::super::sdl::{graphql_sdl_for_role, SdlOptions};

    let mut grants = BTreeMap::new();
    grants.insert(
        "OrderView".to_string(),
        RoleGrant::columns(["order_id", "status"]),
    );
    let sdl = graphql_sdl_for_role(&[orders()], &SdlOptions::sqlite(), "user", &grants)
        .expect("role sdl");

    // Granted
    assert!(
        sdl.contains("order_id") && sdl.contains("status"),
        "expected granted columns in SDL: {sdl}"
    );
    // Ungranted column fields must not appear on the object type body.
    // (meta / customer_id were not granted)
    assert!(
        !sdl.contains("customer_id"),
        "ungranted customer_id leaked into role SDL: {sdl}"
    );
    assert!(
        !sdl.contains("meta"),
        "ungranted meta leaked into role SDL: {sdl}"
    );
    // SQLite: no PG JSON ops even if JSON columns were granted
    for forbidden in ["_contains", "_contained_in", "_has_key"] {
        assert!(
            !sdl.contains(forbidden),
            "SQLite role SDL must not expose {forbidden}"
        );
    }
}

#[test]
fn role_sdl_empty_grants_has_no_query_roots() {
    use super::super::sdl::{graphql_sdl_for_role, SdlOptions};

    let sdl = graphql_sdl_for_role(&[orders()], &SdlOptions::sqlite(), "anon", &BTreeMap::new())
        .expect("empty role sdl");
    // No list roots for orders when model ungranted
    assert!(
        !sdl.contains("orders(") && !sdl.contains("orders:"),
        "empty grants should not expose orders roots: {sdl}"
    );
    assert!(sdl.contains("type Query {\n  _empty: Boolean!\n}"));
}

#[test]
fn constant_validation_uses_exact_wire_scalar_domains() {
    let command = SurfaceCommand {
        command_name: "test.constant".into(),
        field_name: "test_constant".into(),
        roles: Vec::new(),
        input: SurfaceCommandShape::None,
        output: SurfaceCommandShape::Typed(SurfaceTypeDef {
            name: "ConstantPayload".into(),
            fields: vec![SurfaceTypeField {
                name: "ok".into(),
                type_name: "Boolean".into(),
                nullable: false,
                list: false,
                item_nullable: false,
                nested: None,
            }],
        }),
        consistency: CommandConsistency::Succeeded,
        input_defaults: Vec::new(),
        effects: Some(CommandEffects::revalidate()),
        confirmations: Vec::new(),
        projected_model: None,
        direct_projection: None,
        projections: Default::default(),
        confirmation_unavailable: false,
    };
    let constant = |value| EffectExpression::Constant { value };
    let column = |name: &str, scalar: &str| ColumnField {
        name: name.into(),
        scalar: scalar.into(),
        nullable: false,
    };

    for (value, scalar) in [
        (serde_json::json!(1.5), "BigInt"),
        (serde_json::json!(2_147_483_648_i64), "Int"),
        (serde_json::json!("not-a-timestamp"), "Timestamptz"),
        (serde_json::json!("***"), "Bytea"),
    ] {
        assert!(
            validate_effect_expression(&command, &constant(value), &column("value", scalar),)
                .is_err()
        );
    }

    for (value, scalar) in [
        (serde_json::json!(42), "BigInt"),
        (serde_json::json!("2026-07-22T12:30:45.123Z"), "Timestamptz"),
        (serde_json::json!("AQID"), "Bytea"),
    ] {
        assert!(
            validate_effect_expression(&command, &constant(value), &column("value", scalar),)
                .is_ok()
        );
    }
}

#[test]
fn missing_surface_primary_key_column_is_a_configuration_error_not_a_panic() {
    let surface = build_surface(&[orders()], &SurfaceOptions::sqlite()).unwrap();
    let mut model = surface.models["OrderView"].clone();
    model.columns.retain(|column| column.name != "order_id");
    let models = BTreeMap::from([("OrderView".into(), model)]);
    let command = SurfaceCommand {
        command_name: "order.patch".into(),
        field_name: "order_patch".into(),
        roles: Vec::new(),
        input: SurfaceCommandShape::None,
        output: SurfaceCommandShape::Typed(SurfaceTypeDef {
            name: "PatchOrderPayload".into(),
            fields: vec![SurfaceTypeField {
                name: "order_id".into(),
                type_name: "String".into(),
                nullable: false,
                list: false,
                item_nullable: false,
                nested: None,
            }],
        }),
        consistency: CommandConsistency::Succeeded,
        input_defaults: Vec::new(),
        effects: Some(CommandEffects::revalidate()),
        confirmations: Vec::new(),
        projected_model: None,
        direct_projection: None,
        projections: Default::default(),
        confirmation_unavailable: false,
    };
    let key = EffectKey {
        fields: vec![EffectFieldValue {
            field: "order_id".into(),
            value: EffectExpression::Constant {
                value: serde_json::json!("order-1"),
            },
        }],
    };

    let result =
        std::panic::catch_unwind(|| validate_effect_key(&models, &command, "OrderView", &key));
    let error = result
        .expect("malformed Surface metadata must not panic")
        .expect_err("missing primary-key column must fail closed");
    assert!(error.contains("missing or hidden on the selected Surface"));
}

/// A7: role×dialect inventory + IR→SDL ops stay aligned (portable fixture).
#[test]
fn a7_role_dialect_parity_inventory_and_sdl_ops() {
    use super::super::sdl::{graphql_sdl_for_role, SdlOptions};

    let full_sqlite = build_surface(&[orders()], &SurfaceOptions::sqlite()).unwrap();
    let full_pg = build_surface(&[orders()], &SurfaceOptions::postgres()).unwrap();

    // Dialect honesty on full surface
    let sqlite_json = full_sqlite.comparison_ops_for_scalar("JSON");
    let pg_json = full_pg.comparison_ops_for_scalar("JSON");
    assert!(sqlite_json.contains(&"_eq"));
    assert!(!sqlite_json.contains(&"_contains"));
    assert!(pg_json.contains(&"_contains"));

    let mut grants = BTreeMap::new();
    grants.insert(
        "OrderView".to_string(),
        RoleGrant::columns(["order_id", "status"]),
    );

    for (opts, dialect_label) in [
        (SdlOptions::sqlite(), "sqlite"),
        (SdlOptions::postgres(), "postgres"),
    ] {
        let full = build_surface(
            &[orders()],
            &SurfaceOptions {
                dialect: if opts.jsonb_operators {
                    SurfaceDialect::Postgres
                } else {
                    SurfaceDialect::Sqlite
                },
                aggregates: opts.aggregates,
                subscriptions: opts.subscriptions,
                default_limit: 100,
                max_limit: 1000,
            },
        )
        .unwrap();
        let role_s = surface_for_role(&full, "user", &grants).unwrap();
        let roots: Vec<_> = role_s.query_root_names();
        assert!(
            roots.contains(&"orders") && roots.contains(&"orders_by_pk"),
            "{dialect_label}: missing list/by_pk roots {roots:?}"
        );
        assert!(
            !roots.iter().any(|n| n.contains("aggregate")),
            "{dialect_label}: aggregate without grant"
        );
        let cols: Vec<_> = role_s
            .models
            .get("OrderView")
            .unwrap()
            .columns
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(cols, vec!["order_id", "status"]);

        let sdl = graphql_sdl_for_role(&[orders()], &opts, "user", &grants).unwrap();
        assert!(
            sdl.contains("order_id") && !sdl.contains("customer_id"),
            "{dialect_label}: SDL column leak: {sdl}"
        );
        // SQLite role SDL never exposes PG JSON ops even if dialect flag wrong on unused scalars
        if !opts.jsonb_operators {
            for forbidden in ["_contains", "_contained_in", "_has_key"] {
                assert!(
                    !sdl.contains(forbidden),
                    "{dialect_label}: {forbidden} in SDL"
                );
            }
        }
    }
}
