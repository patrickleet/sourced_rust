use super::*;
use crate::graphql::{
    build_surface, claim, col, rel, surface_for_application, surface_for_role, typed_command,
    GraphqlInputType, GraphqlOutputType, GraphqlTypeDef, GraphqlTypeField, PreparedCommand,
    RoleGrant, Succeeded, SurfaceCommand, SurfaceOptions, SurfaceProjector, SurfaceTypeField,
};
use crate::microsvc::{CausalCommandContext, HandlerError, Routes, Service};
use crate::table::{
    ColumnType, PrimaryKey, RelationshipDef, RelationshipKind, TableColumn, TableKind, TableSchema,
};
use std::any::TypeId;

#[test]
fn generated_causal_command_operation_requires_framework_command_id() {
    let operation = command_operation(
        "todo_create",
        &ClientCommandShape::Object {
            definition: ClientTypeDef {
                name: "CreateTodoInput".into(),
                fields: Vec::new(),
            },
        },
        &ClientCommandShape::Object {
            definition: ClientTypeDef {
                name: "CreateTodoOutput".into(),
                fields: vec![ClientTypeField {
                    name: "id".into(),
                    type_name: "String".into(),
                    nullable: false,
                    list: false,
                    item_nullable: false,
                    codec: Some("string".into()),
                    nested: None,
                }],
            },
        },
    );
    assert_eq!(
            operation,
            "mutation Client_todo_create($commandId: ID!, $input: CreateTodoInput!) { todo_create(commandId: $commandId, input: $input) { id } }"
        );
}

fn column(name: &str, ty: ColumnType) -> TableColumn {
    TableColumn::new(name, name, ty)
}

fn users() -> TableSchema {
    TableSchema {
        model_name: "UserView".into(),
        table_name: "users".into(),
        columns: vec![
            TableColumn {
                primary_key: true,
                ..column("user_id", ColumnType::Text)
            },
            column("display_name", ColumnType::Text),
            column("secret", ColumnType::Text),
        ],
        primary_key: PrimaryKey::new(["user_id"]),
        version_column: Some("_sourced_version".into()),
        foreign_keys: Vec::new(),
        indexes: Vec::new(),
        relationships: Vec::new(),
        kind: TableKind::ReadModel,
    }
}

fn todos() -> TableSchema {
    TableSchema {
        model_name: "TodoView".into(),
        table_name: "todos".into(),
        columns: vec![
            TableColumn {
                primary_key: true,
                ..column("todo_id", ColumnType::Text)
            },
            column("owner_id", ColumnType::Text),
            column("title", ColumnType::Text),
            column("completed", ColumnType::Boolean),
        ],
        primary_key: PrimaryKey::new(["todo_id"]),
        version_column: Some("_sourced_version".into()),
        foreign_keys: Vec::new(),
        indexes: Vec::new(),
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

fn memberships() -> TableSchema {
    TableSchema {
        model_name: "MembershipView".into(),
        table_name: "memberships".into(),
        columns: vec![
            TableColumn {
                primary_key: true,
                ..column("tenant_id", ColumnType::Text)
            },
            TableColumn {
                primary_key: true,
                ..column("user_id", ColumnType::Text)
            },
            column("role", ColumnType::Text),
        ],
        primary_key: PrimaryKey::new(["tenant_id", "user_id"]),
        version_column: None,
        foreign_keys: Vec::new(),
        indexes: Vec::new(),
        relationships: Vec::new(),
        kind: TableKind::ReadModel,
    }
}

fn teams() -> TableSchema {
    TableSchema {
        model_name: "TeamView".into(),
        table_name: "teams".into(),
        columns: vec![
            TableColumn {
                primary_key: true,
                ..column("team_id", ColumnType::Text)
            },
            column("name", ColumnType::Text),
        ],
        primary_key: PrimaryKey::new(["team_id"]),
        version_column: None,
        foreign_keys: Vec::new(),
        indexes: Vec::new(),
        relationships: vec![RelationshipDef {
            field_name: "members".into(),
            kind: RelationshipKind::ManyToMany,
            target_model: "UserView".into(),
            foreign_key: Some("team_id".into()),
            through: Some("private_team_members".into()),
            target_foreign_key: Some("user_id".into()),
        }],
        kind: TableKind::ReadModel,
    }
}

fn team_members() -> TableSchema {
    TableSchema {
        model_name: "PrivateTeamMember".into(),
        table_name: "private_team_members".into(),
        columns: vec![
            TableColumn {
                primary_key: true,
                ..column("team_id", ColumnType::Text)
            },
            TableColumn {
                primary_key: true,
                ..column("user_id", ColumnType::Text)
            },
        ],
        primary_key: PrimaryKey::new(["team_id", "user_id"]),
        version_column: None,
        foreign_keys: Vec::new(),
        indexes: Vec::new(),
        relationships: Vec::new(),
        kind: TableKind::Operational,
    }
}

#[derive(Deserialize)]
struct CompleteInput;
impl GraphqlInputType for CompleteInput {
    fn graphql_type() -> GraphqlTypeDef {
        GraphqlTypeDef::new(
            "CompleteTodoInput",
            vec![GraphqlTypeField {
                name: "todo_id".into(),
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
struct CompletePayload;
impl GraphqlOutputType for CompletePayload {
    fn graphql_type() -> GraphqlTypeDef {
        GraphqlTypeDef::new(
            "CompleteTodoPayload",
            vec![GraphqlTypeField {
                name: "todo_id".into(),
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

#[derive(Default)]
struct ManifestAggregate {
    entity: crate::Entity,
}

impl crate::Aggregate for ManifestAggregate {
    type ReplayError = std::convert::Infallible;

    fn entity(&self) -> &crate::Entity {
        &self.entity
    }

    fn entity_mut(&mut self) -> &mut crate::Entity {
        &mut self.entity
    }

    fn replay_event(&mut self, _event: &crate::EventRecord) -> Result<(), Self::ReplayError> {
        Ok(())
    }
}

async fn complete_handler(
    _context: &CausalCommandContext<'_, ManifestAggregate>,
    _input: CompleteInput,
) -> Result<PreparedCommand<Succeeded<CompletePayload>>, HandlerError> {
    Ok(
        PreparedCommand::<Succeeded<CompletePayload>>::prepare(CompletePayload)
            .expect("serializable command payload"),
    )
}

fn full_surface() -> Surface {
    let service = Service::new().named("todos-service").routes(
        Routes::new()
            .with_repo(crate::AggregateRepository::<_, ManifestAggregate>::new(
                crate::InMemoryRepository::new(),
            ))
            .typed_command(
                typed_command::<CompleteInput, Succeeded<CompletePayload>>("todo.complete")
                    .field_name("todos_complete")
                    .roles(["admin", "user"]),
            )
            .handle(complete_handler)
            .typed_command(
                typed_command::<CompleteInput, Succeeded<CompletePayload>>("todo.force_archive")
                    .field_name("todos_force_archive")
                    .roles(["admin"]),
            )
            .handle(complete_handler),
    );
    build_surface(
        &[todos(), users(), memberships()],
        &SurfaceOptions::sqlite(),
    )
    .expect("surface")
    .with_service(&service)
    .expect("typed service")
    .with_projectors([
        SurfaceProjector::new("project_todos")
            .facts(["todo.completed"])
            .models(["TodoView"]),
        SurfaceProjector::new("project_users")
            .facts(["user.changed"])
            .models(["UserView"]),
    ])
    .expect("projectors")
}

fn projected_surface() -> Surface {
    use super::super::command_contract::{CommandEffects, CommandProjectedModel, EffectExpression};

    let todo_schema: &'static TableSchema = Box::leak(Box::new(todos()));
    let mut surface = build_surface(&[todo_schema.clone(), users()], &SurfaceOptions::sqlite())
        .expect("projected surface");
    let todo_model = surface.models["TodoView"].clone();
    let mut output_fields = todo_model
        .columns
        .iter()
        .map(|column| SurfaceTypeField {
            name: column.name.clone(),
            type_name: column.scalar.clone(),
            nullable: column.nullable,
            list: false,
            item_nullable: false,
            nested: None,
        })
        .collect::<Vec<_>>();
    output_fields.sort_by(|left, right| left.name.cmp(&right.name));
    surface.commands = vec![SurfaceCommand {
        command_name: "todo.project".into(),
        field_name: "todo_project".into(),
        roles: vec!["user".into()],
        input: SurfaceCommandShape::Typed(SurfaceTypeDef {
            name: "ProjectTodoInput".into(),
            fields: vec![SurfaceTypeField {
                name: "todo_id".into(),
                type_name: "String".into(),
                nullable: false,
                list: false,
                item_nullable: false,
                nested: None,
            }],
        }),
        output: SurfaceCommandShape::Typed(SurfaceTypeDef {
            name: todo_model.object_name,
            fields: output_fields,
        }),
        consistency: CommandConsistency::Projected,
        input_defaults: Vec::new(),
        effects: Some(CommandEffects::revalidate()),
        confirmations: Vec::new(),
        projected_model: Some(CommandProjectedModel {
            output_type_id: TypeId::of::<()>(),
            model: "TodoView".into(),
            table: "todos".into(),
            schema: todo_schema,
            partition: Some(EffectExpression::Input {
                path: vec!["todo_id".into()],
            }),
        }),
        direct_projection: None,
        projections: Default::default(),
        confirmation_unavailable: false,
    }];
    surface.commands_attached = true;
    surface
        .with_projectors([SurfaceProjector::new("project_todo_domain")
            .facts(["todo.changed", "user.changed"])
            .models(["TodoView", "UserView"])
            .partition_by(["todo_id"])
            .change_epoch("todo-domain-v1")])
        .expect("projected topology")
}

#[test]
fn projected_command_exports_opaque_role_safe_direct_target() {
    let full = projected_surface();
    let selected = surface_for_role(
        &full,
        "user",
        &BTreeMap::from([("TodoView".into(), RoleGrant::all_columns())]),
    )
    .expect("role selection");
    assert!(
        selected.projectors.is_empty(),
        "the multi-model owner must be omitted when one owned model is denied"
    );

    let manifest = client_manifest_from_surface(
        "todos-service",
        ClientSurfaceIdentity::role("user"),
        &selected,
    )
    .expect("projected client manifest");
    let direct = manifest.commands[0]
        .extensions
        .direct_projection
        .as_ref()
        .expect("projected direct target");
    assert_eq!(direct.topology.version, 1);
    assert_eq!(direct.topology.name, "project_todo_domain");
    assert_eq!(direct.topology.digest.len(), 71);
    assert!(direct.topology.digest.starts_with("sha256:"));
    assert!(direct.topology.digest[7..]
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
    assert_eq!(direct.model, "TodoView");
    assert_eq!(
        direct.partition,
        Some(serde_json::json!({"kind": "input", "path": ["todo_id"]}))
    );
    assert_eq!(direct.change_epoch, "todo-domain-v1");

    let direct_wire = serde_json::to_value(direct).expect("direct target wire");
    assert_eq!(
        direct_wire
            .as_object()
            .expect("direct target object")
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["change_epoch", "model", "partition", "topology"])
    );
    let wire = serde_json::to_string(&manifest).expect("client manifest wire");
    assert!(!wire.contains("UserView"));
    assert!(!wire.contains("users"));
    assert!(!wire.contains("user.changed"));
}

#[test]
fn trusted_preset_artifacts_expose_typed_descriptors_without_values() {
    use super::super::command_contract::{
        CommandEffect, CommandEffects, EffectExpression, EffectFieldValue, EffectKey,
    };

    let mut full = projected_surface();
    full.commands[0].effects = Some(CommandEffects::new([CommandEffect::Patch {
        model: "TodoView".into(),
        key: EffectKey {
            fields: vec![EffectFieldValue {
                field: "todo_id".into(),
                value: EffectExpression::Input {
                    path: vec!["todo_id".into()],
                },
            }],
        },
        fields: vec![EffectFieldValue {
            field: "owner_id".into(),
            value: EffectExpression::TrustedPreset {
                name: "x-user-id".into(),
            },
        }],
    }]));
    let selected = surface_for_role(
        &full,
        "user",
        &BTreeMap::from([
            ("TodoView".into(), RoleGrant::all_columns()),
            ("UserView".into(), RoleGrant::all_columns()),
        ]),
    )
    .expect("role selection");
    let manifest = client_manifest_from_surface(
        "todos-service",
        ClientSurfaceIdentity::role("user"),
        &selected,
    )
    .expect("trusted preset descriptor manifest");

    assert_eq!(
        manifest.commands[0].extensions.trusted_presets,
        vec![ClientTrustedPresetDescriptor {
            name: "x-user-id".into(),
            codec: "string".into(),
        }]
    );
    let wire = serde_json::to_value(manifest).expect("manifest JSON");
    assert_eq!(
        wire["commands"][0]["extensions"]["trusted_presets"],
        serde_json::json!([{"name": "x-user-id", "codec": "string"}])
    );
    assert!(
        !wire.to_string().contains("preset-value"),
        "static artifacts must never freeze a runtime preset value"
    );
}

#[test]
fn projected_command_export_rejects_absent_or_wrong_direct_target() {
    let full = projected_surface();
    let mut selected = surface_for_role(
        &full,
        "user",
        &BTreeMap::from([
            ("TodoView".into(), RoleGrant::all_columns()),
            ("UserView".into(), RoleGrant::all_columns()),
        ]),
    )
    .expect("role selection");

    selected.commands[0].direct_projection = None;
    let error = client_manifest_from_surface(
        "todos-service",
        ClientSurfaceIdentity::role("user"),
        &selected,
    )
    .expect_err("projected command cannot omit direct target");
    assert!(error
        .0
        .contains("missing its bound direct projection target"));

    let mut selected = surface_for_role(
        &full,
        "user",
        &BTreeMap::from([
            ("TodoView".into(), RoleGrant::all_columns()),
            ("UserView".into(), RoleGrant::all_columns()),
        ]),
    )
    .expect("role selection");
    selected.commands[0]
        .direct_projection
        .as_mut()
        .expect("bound target")
        .model = "UserView".into();
    let error = client_manifest_from_surface(
        "todos-service",
        ClientSurfaceIdentity::role("user"),
        &selected,
    )
    .expect_err("direct target cannot name a different retained model");
    assert!(error.0.contains("differs from retained model"));
}

fn grants() -> BTreeMap<String, BTreeMap<String, RoleGrant>> {
    BTreeMap::from([
        (
            "admin".into(),
            BTreeMap::from([
                (
                    "TodoView".into(),
                    RoleGrant::all_columns().with_aggregations(),
                ),
                (
                    "UserView".into(),
                    RoleGrant::all_columns().with_aggregations(),
                ),
                ("MembershipView".into(), RoleGrant::all_columns()),
            ]),
        ),
        (
            "user".into(),
            BTreeMap::from([
                (
                    "TodoView".into(),
                    RoleGrant::all_columns()
                        .rows(col("owner_id").eq(claim("x-user-id")))
                        .limit(25),
                ),
                ("UserView".into(), RoleGrant::columns(["display_name"])),
                (
                    "MembershipView".into(),
                    RoleGrant::columns(["tenant_id", "role"]),
                ),
            ]),
        ),
    ])
}

fn manifest_for_all_models(
    service_id: &str,
    role: &str,
    full: &Surface,
) -> DistributedClientManifest {
    let grants = full
        .models
        .keys()
        .map(|model| (model.clone(), RoleGrant::all_columns().with_aggregations()))
        .collect();
    let selected = surface_for_role(full, role, &grants).expect("role surface");
    client_manifest_from_surface(service_id, ClientSurfaceIdentity::role(role), &selected)
        .expect("client manifest")
}

#[test]
fn role_manifest_is_deterministic_and_hides_denied_identity_and_commands() {
    let full = full_surface();
    let selected = surface_for_role(&full, "user", &grants()["user"]).unwrap();
    let export = DistributedClientSurfaceExport::from_selected("todos-service", selected)
        .expect("role-selected Surface");
    let first = export.manifest().unwrap();
    let second = export.manifest().unwrap();
    assert_eq!(first, second);
    assert_eq!(first.manifest_version, 1);
    assert_eq!(first.schema_fingerprint, second.schema_fingerprint);
    assert_eq!(
        first.schema_fingerprint,
        "sha256:d2a890402634708d74f3edd895b7985fa0e3a86f6787730798b7d8c94f078db8"
    );
    assert_eq!(
        first.protocol_fingerprint,
        "sha256:30f19c9f4d29280a02ddf67c4df62cdc92c4e8090792f43d6b1bdafea3e31273"
    );

    let user = first
        .models
        .iter()
        .find(|model| model.id == "UserView")
        .unwrap();
    assert_eq!(user.normalization, ModelNormalization::Embedded);
    assert!(user.record_revisions && user.tombstones);
    assert_eq!(
        user.fields
            .iter()
            .map(|field| field.name.as_str())
            .collect::<Vec<_>>(),
        vec!["display_name"]
    );
    let todo = first
        .models
        .iter()
        .find(|model| model.id == "TodoView")
        .unwrap();
    assert!(todo.record_revisions && todo.tombstones);
    assert_eq!(
        todo.row_policy,
        ClientRowPolicy::Predicate {
            expression: col("owner_id").eq(claim("x-user-id")),
        }
    );
    assert_eq!(
        trusted_preset_descriptors(&first).unwrap(),
        vec![ClientTrustedPresetDescriptor {
            name: "x-user-id".into(),
            codec: "string".into(),
        }]
    );
    let owner = todo
        .relationships
        .iter()
        .find(|rel| rel.name == "owner")
        .unwrap();
    assert!(!owner.nullable);
    assert_eq!(owner.key_mapping, RelationshipKeyMapping::Embedded);
    assert_eq!(owner.maintenance, ClientRelationshipMaintenance::Revalidate);
    assert_eq!(owner.dependencies, vec!["todos", "users"]);
    assert!(
        !owner.live,
        "singular relationships are not live list plans"
    );
    assert!(owner.arguments.is_empty());
    assert!(owner.filter.is_none());
    assert_eq!(todo.filter_input.type_name, "todos_bool_exp");
    assert_eq!(
        todo.filter_input
            .relationships
            .iter()
            .find(|relationship| relationship.field == "owner")
            .expect("singular relationship predicate")
            .target_type,
        user.filter_input.type_name
    );
    assert_eq!(user.filter_input.type_name, "users_bool_exp");
    assert!(first.capabilities.live_queries);
    assert!(!first.capabilities.record_revisions);
    assert!(!first.capabilities.tombstones);
    assert!(!first.capabilities.live_resume);
    assert_eq!(first.capabilities.query_fallback, "revalidate");
    assert!(first.capabilities.causal_receipts);
    assert!(first.capabilities.cache_scope);
    let membership = first
        .models
        .iter()
        .find(|model| model.id == "MembershipView")
        .unwrap();
    assert!(!membership.record_revisions && !membership.tombstones);
    let status = first
        .protocol_operations
        .command_status
        .as_ref()
        .expect("causal surfaces generate the framework status operation");
    assert_eq!(status.name, "Distributed_CommandStatus");
    assert_eq!(status.operation, command_status_operation());
    assert_eq!(
        status.operation_hash,
        hash_bytes(status.operation.as_bytes())
    );
    assert!(first
        .commands
        .iter()
        .any(|command| command.name == "todo.complete"));
    assert!(!first
        .commands
        .iter()
        .any(|command| command.name == "todo.force_archive"));
    assert_eq!(first.commands[0].grants, vec!["user"]);
    assert!(first.commands.iter().all(|command| {
        command.extensions.consistency.kind == "succeeded"
            && command.extensions.effects.as_ref().is_some_and(|effects| {
                effects.operations.is_empty() && effects.fallback == "revalidate"
            })
            && command.extensions.confirmations.is_none()
    }));

    let json = serde_json::to_string(&first).unwrap();
    assert!(!json.contains("secret"));
    assert!(!json.contains("user_id"));
    assert!(!json.contains("force_archive"));
    assert!(
        json.contains("x-user-id"),
        "portable row policies expose only the static claim name, never its value"
    );
}

#[test]
fn filter_execution_limits_are_schema_fingerprinted_without_changing_protocol_epoch() {
    let full = full_surface();
    let selected = surface_for_role(&full, "user", &grants()["user"]).unwrap();
    let baseline = DistributedClientSurfaceExport::from_selected("todos-service", selected.clone())
        .unwrap()
        .manifest()
        .unwrap();

    let mut bool_limits = ClientExecutionLimits::default();
    bool_limits.max_bool_width += 1;
    let bool_manifest = DistributedClientSurfaceExport::from_selected_with_execution(
        "todos-service",
        selected.clone(),
        bool_limits,
    )
    .unwrap()
    .manifest()
    .unwrap();

    let mut in_limits = ClientExecutionLimits::default();
    in_limits.max_in_list += 1;
    let in_manifest = DistributedClientSurfaceExport::from_selected_with_execution(
        "todos-service",
        selected,
        in_limits,
    )
    .unwrap()
    .manifest()
    .unwrap();

    assert_ne!(
        baseline.schema_fingerprint,
        bool_manifest.schema_fingerprint
    );
    assert_ne!(baseline.schema_fingerprint, in_manifest.schema_fingerprint);
    assert_ne!(
        bool_manifest.schema_fingerprint,
        in_manifest.schema_fingerprint
    );
    assert_eq!(baseline.protocol_version, 1);
    assert_eq!(
        baseline.protocol_fingerprint,
        bool_manifest.protocol_fingerprint
    );
    assert_eq!(
        baseline.protocol_fingerprint,
        in_manifest.protocol_fingerprint
    );
}

#[test]
fn relationship_nullability_is_copied_from_the_authoritative_surface() {
    let mut fingerprints = Vec::new();
    for nullable in [false, true] {
        let mut todo_schema = todos();
        todo_schema
            .columns
            .iter_mut()
            .find(|column| column.column_name == "owner_id")
            .expect("owner foreign key")
            .nullable = nullable;
        let full = build_surface(&[todo_schema, users()], &SurfaceOptions::sqlite()).unwrap();
        let selected = surface_for_role(
            &full,
            "user",
            &BTreeMap::from([
                ("TodoView".into(), RoleGrant::all_columns()),
                ("UserView".into(), RoleGrant::all_columns()),
            ]),
        )
        .unwrap();
        let surface_nullable = selected.models["TodoView"]
            .relationships
            .iter()
            .find(|relationship| relationship.name == "owner")
            .expect("surface relationship")
            .nullable;
        assert_eq!(surface_nullable, nullable);

        let manifest = client_manifest_from_surface(
            "todos-service",
            ClientSurfaceIdentity::role("user"),
            &selected,
        )
        .unwrap();
        fingerprints.push(manifest.schema_fingerprint.clone());
        let owner = manifest
            .models
            .iter()
            .find(|model| model.id == "TodoView")
            .unwrap()
            .relationships
            .iter()
            .find(|relationship| relationship.name == "owner")
            .unwrap();
        assert_eq!(owner.nullable, surface_nullable);
        assert_eq!(serde_json::to_value(owner).unwrap()["nullable"], nullable);
    }
    assert_ne!(
        fingerprints[0], fingerprints[1],
        "relationship nullability is part of the schema fingerprint"
    );
}

#[test]
fn query_protocol_capabilities_are_complete_or_explicitly_revalidate() {
    let fully_owned = build_surface(&[todos(), users()], &SurfaceOptions::sqlite())
        .unwrap()
        .with_projectors([
            SurfaceProjector::new("project_todos")
                .facts(["todo.changed"])
                .models(["TodoView"])
                .change_epoch("todos-v1"),
            SurfaceProjector::new("project_users")
                .facts(["user.changed"])
                .models(["UserView"])
                .partition_constant(serde_json::json!({"scope": "all"}))
                .change_epoch("users-v1"),
        ])
        .unwrap();
    let fully_owned = manifest_for_all_models("query-capabilities", "user", &fully_owned);
    assert!(fully_owned.capabilities.record_revisions);
    assert!(fully_owned.capabilities.tombstones);
    assert!(fully_owned.capabilities.live_resume);
    assert_eq!(fully_owned.capabilities.query_fallback, "revalidate");
    assert!(fully_owned
        .models
        .iter()
        .all(|model| model.record_revisions && model.tombstones));
    let wire = serde_json::to_value(&fully_owned).unwrap();
    assert_eq!(wire["capabilities"]["record_revisions"], true);
    assert_eq!(wire["capabilities"]["query_fallback"], "revalidate");
    assert!(wire["capabilities"].get("framework_revisions").is_none());
    assert!(wire["models"][0].get("framework_revision").is_none());

    let dynamic = build_surface(&[users()], &SurfaceOptions::sqlite())
        .unwrap()
        .with_projectors([SurfaceProjector::new("project_users")
            .facts(["user.changed"])
            .models(["UserView"])
            .partition_by(["tenant_id"])
            .change_epoch("users-v1")])
        .unwrap();
    let dynamic = manifest_for_all_models("query-capabilities", "user", &dynamic);
    assert!(dynamic.capabilities.record_revisions);
    assert!(dynamic.capabilities.tombstones);
    assert!(!dynamic.capabilities.live_resume);
    assert_eq!(dynamic.capabilities.query_fallback, "revalidate");

    let row_filtered = build_surface(&[users()], &SurfaceOptions::sqlite())
        .unwrap()
        .with_projectors([SurfaceProjector::new("project_users")
            .facts(["user.changed"])
            .models(["UserView"])
            .change_epoch("users-v1")])
        .unwrap();
    let row_filtered = surface_for_role(
        &row_filtered,
        "user",
        &BTreeMap::from([(
            "UserView".into(),
            RoleGrant::all_columns().rows(col("user_id").eq("visible-user")),
        )]),
    )
    .unwrap();
    let row_filtered = client_manifest_from_surface(
        "query-capabilities",
        ClientSurfaceIdentity::role("user"),
        &row_filtered,
    )
    .unwrap();
    assert!(row_filtered.capabilities.record_revisions);
    assert!(row_filtered.capabilities.tombstones);
    assert!(
        !row_filtered.capabilities.live_resume,
        "partition-wide positions and changes must not cross a row-authorization boundary"
    );

    let epochless = build_surface(&[users()], &SurfaceOptions::sqlite())
        .unwrap()
        .with_projectors([SurfaceProjector::new("project_users")
            .facts(["user.changed"])
            .models(["UserView"])])
        .unwrap();
    let epochless = manifest_for_all_models("query-capabilities", "user", &epochless);
    assert!(epochless.capabilities.record_revisions);
    assert!(epochless.capabilities.tombstones);
    assert!(!epochless.capabilities.live_resume);
    assert_eq!(epochless.capabilities.query_fallback, "revalidate");

    let unowned = build_surface(&[users()], &SurfaceOptions::sqlite()).unwrap();
    let unowned = manifest_for_all_models("query-capabilities", "user", &unowned);
    assert!(!unowned.capabilities.record_revisions);
    assert!(!unowned.capabilities.tombstones);
    assert!(!unowned.capabilities.live_resume);
    assert_eq!(unowned.capabilities.query_fallback, "revalidate");
    assert!(unowned
        .models
        .iter()
        .all(|model| !model.record_revisions && !model.tombstones));

    let mixed = build_surface(&[todos(), users()], &SurfaceOptions::sqlite())
        .unwrap()
        .with_projectors([SurfaceProjector::new("project_todos")
            .facts(["todo.changed"])
            .models(["TodoView"])
            .change_epoch("todos-v1")])
        .unwrap();
    let mixed = manifest_for_all_models("query-capabilities", "user", &mixed);
    assert!(!mixed.capabilities.record_revisions);
    assert!(!mixed.capabilities.tombstones);
    assert!(!mixed.capabilities.live_resume);
    assert_eq!(mixed.capabilities.query_fallback, "revalidate");
    assert!(mixed
        .models
        .iter()
        .find(|model| model.id == "TodoView")
        .is_some_and(|model| model.record_revisions && model.tombstones));
    assert!(mixed
        .models
        .iter()
        .find(|model| model.id == "UserView")
        .is_some_and(|model| !model.record_revisions && !model.tombstones));

    let uncovered_join = build_surface(
        &[teams(), users(), team_members()],
        &SurfaceOptions::sqlite(),
    )
    .unwrap()
    .with_projectors([
        SurfaceProjector::new("project_teams")
            .facts(["team.changed"])
            .models(["TeamView"])
            .change_epoch("teams-v1"),
        SurfaceProjector::new("project_users")
            .facts(["user.changed"])
            .models(["UserView"])
            .change_epoch("users-v1"),
    ])
    .unwrap();
    let uncovered_join = manifest_for_all_models("query-capabilities", "user", &uncovered_join);
    assert!(uncovered_join.capabilities.record_revisions);
    assert!(uncovered_join.capabilities.tombstones);
    assert!(!uncovered_join.capabilities.live_resume);
    assert_eq!(uncovered_join.capabilities.query_fallback, "revalidate");

    let mut query_only_options = SurfaceOptions::sqlite();
    query_only_options.subscriptions = false;
    let query_only = build_surface(&[users()], &query_only_options)
        .unwrap()
        .with_projectors([SurfaceProjector::new("project_users")
            .facts(["user.changed"])
            .models(["UserView"])
            .change_epoch("users-v1")])
        .unwrap();
    let query_only = manifest_for_all_models("query-capabilities", "user", &query_only);
    assert!(query_only.capabilities.record_revisions);
    assert!(query_only.capabilities.tombstones);
    assert!(!query_only.capabilities.live_queries);
    assert!(!query_only.capabilities.live_resume);
    assert_eq!(query_only.capabilities.query_fallback, "revalidate");

    let empty = build_surface(&[], &SurfaceOptions::sqlite()).unwrap();
    let empty = manifest_for_all_models("query-capabilities", "user", &empty);
    assert!(!empty.capabilities.record_revisions);
    assert!(!empty.capabilities.tombstones);
    assert!(!empty.capabilities.live_resume);
    assert_eq!(empty.capabilities.query_fallback, "revalidate");
}

#[test]
fn composite_keys_normalize_in_declared_order_and_hidden_keys_embed() {
    let full = full_surface();
    let admin = surface_for_role(&full, "admin", &grants()["admin"]).unwrap();
    let admin_manifest = client_manifest_from_surface(
        "todos-service",
        ClientSurfaceIdentity::role("admin"),
        &admin,
    )
    .unwrap();
    let membership = admin_manifest
        .models
        .iter()
        .find(|model| model.id == "MembershipView")
        .unwrap();
    let ModelNormalization::Normalized { fields, encoding } = &membership.normalization else {
        panic!("composite model should normalize")
    };
    assert_eq!(
        fields
            .iter()
            .map(|field| field.name.as_str())
            .collect::<Vec<_>>(),
        vec!["tenant_id", "user_id"]
    );
    assert_eq!(encoding, KEY_ENCODING);

    let user = surface_for_role(&full, "user", &grants()["user"]).unwrap();
    let user_manifest =
        client_manifest_from_surface("todos-service", ClientSurfaceIdentity::role("user"), &user)
            .unwrap();
    let membership = user_manifest
        .models
        .iter()
        .find(|model| model.id == "MembershipView")
        .unwrap();
    assert_eq!(membership.normalization, ModelNormalization::Embedded);
    assert!(!serde_json::to_string(membership)
        .unwrap()
        .contains("user_id"));
}

#[test]
fn bigint_keys_embed_until_decimal_string_identity_is_available() {
    let accounts = TableSchema {
        model_name: "AccountView".into(),
        table_name: "accounts".into(),
        columns: vec![TableColumn {
            primary_key: true,
            ..column("account_id", ColumnType::Integer)
        }],
        primary_key: PrimaryKey::new(["account_id"]),
        version_column: None,
        foreign_keys: Vec::new(),
        indexes: Vec::new(),
        relationships: Vec::new(),
        kind: TableKind::ReadModel,
    };
    let invoices = TableSchema {
        model_name: "InvoiceView".into(),
        table_name: "invoices".into(),
        columns: vec![
            TableColumn {
                primary_key: true,
                ..column("invoice_id", ColumnType::Text)
            },
            column("account_id", ColumnType::Integer),
        ],
        primary_key: PrimaryKey::new(["invoice_id"]),
        version_column: None,
        foreign_keys: Vec::new(),
        indexes: Vec::new(),
        relationships: vec![RelationshipDef {
            field_name: "account".into(),
            kind: RelationshipKind::BelongsTo,
            target_model: "AccountView".into(),
            foreign_key: Some("account_id".into()),
            through: None,
            target_foreign_key: None,
        }],
        kind: TableKind::ReadModel,
    };
    let full = build_surface(&[accounts, invoices], &SurfaceOptions::sqlite()).unwrap();
    let selected = surface_for_role(
        &full,
        "user",
        &BTreeMap::from([
            (
                "AccountView".into(),
                RoleGrant::all_columns().rows(col("account_id").eq(9_007_199_254_740_992_i64)),
            ),
            ("InvoiceView".into(), RoleGrant::all_columns()),
        ]),
    )
    .unwrap();
    let manifest = DistributedClientSurfaceExport::from_selected("billing", selected)
        .unwrap()
        .manifest()
        .unwrap();

    let account = manifest
        .models
        .iter()
        .find(|model| model.id == "AccountView")
        .unwrap();
    assert_eq!(account.normalization, ModelNormalization::Embedded);
    assert_eq!(account.row_policy, ClientRowPolicy::ServerOnly);
    assert!(!serde_json::to_string(account)
        .unwrap()
        .contains("9007199254740992"));
    let relationship = manifest
        .models
        .iter()
        .find(|model| model.id == "InvoiceView")
        .unwrap()
        .relationships
        .iter()
        .find(|relationship| relationship.name == "account")
        .unwrap();
    assert_eq!(relationship.key_mapping, RelationshipKeyMapping::Embedded);
    assert_eq!(
        relationship.maintenance,
        ClientRelationshipMaintenance::Revalidate
    );
}

#[test]
fn application_surface_is_common_contract_with_safe_role_limit_semantics() {
    let full = full_surface();
    let all_grants = grants();
    let selected =
        surface_for_application(&full, "web", &["user".into(), "admin".into()], &all_grants)
            .unwrap();
    let manifest = client_manifest_from_surface(
        "todos-service",
        ClientSurfaceIdentity::application("web", ["admin", "user"]),
        &selected,
    )
    .unwrap();
    assert!(!manifest
        .commands
        .iter()
        .any(|command| command.name == "todo.force_archive"));
    let todos = manifest
        .roots
        .iter()
        .find(|root| root.id == "query:todos")
        .unwrap();
    assert_eq!(todos.pagination.as_ref().unwrap().default_limit, 25);
    assert_eq!(todos.pagination.as_ref().unwrap().max_limit, 25);
    assert!(matches!(
        todos.filter.as_ref().unwrap().row_policy,
        ClientRowPolicy::ServerOnly
    ));

    let admin = surface_for_role(&full, "admin", &all_grants["admin"]).unwrap();
    let admin_manifest = client_manifest_from_surface(
        "todos-service",
        ClientSurfaceIdentity::role("admin"),
        &admin,
    )
    .unwrap();
    let admin_todos = admin_manifest
        .roots
        .iter()
        .find(|root| root.id == "query:todos")
        .unwrap();
    assert_eq!(admin_todos.pagination.as_ref().unwrap().default_limit, 100);
    assert_eq!(admin_todos.pagination.as_ref().unwrap().max_limit, 1000);
    assert_ne!(
        manifest.schema_fingerprint,
        admin_manifest.schema_fingerprint
    );
    assert_eq!(
        manifest.protocol_fingerprint,
        admin_manifest.protocol_fingerprint
    );
}

#[test]
fn mixed_target_projectors_are_omitted_for_role_and_application_surfaces() {
    let full = build_surface(&[todos(), users(), teams()], &SurfaceOptions::sqlite())
        .unwrap()
        .with_projectors([
            SurfaceProjector::new("project_todos")
                .facts(["todo.changed"])
                .models(["TodoView"]),
            SurfaceProjector::new("project_user_team")
                .facts(["private-user.changed"])
                .models(["UserView", "TeamView"]),
        ])
        .unwrap();
    let restricted = BTreeMap::from([
        ("TodoView".into(), RoleGrant::all_columns()),
        ("UserView".into(), RoleGrant::all_columns()),
    ]);
    let admin = BTreeMap::from([
        ("TodoView".into(), RoleGrant::all_columns()),
        ("UserView".into(), RoleGrant::all_columns()),
        ("TeamView".into(), RoleGrant::all_columns()),
    ]);

    let role = surface_for_role(&full, "restricted", &restricted).unwrap();
    let role_manifest = DistributedClientSurfaceExport::from_selected("todos-service", role)
        .unwrap()
        .manifest()
        .unwrap();
    assert_eq!(
        role_manifest
            .projectors
            .iter()
            .map(|projector| projector.name.as_str())
            .collect::<Vec<_>>(),
        vec!["project_todos"]
    );
    assert!(!role_manifest.capabilities.record_revisions);
    assert!(!role_manifest.capabilities.tombstones);
    assert!(!role_manifest.capabilities.live_resume);
    assert!(role_manifest
        .models
        .iter()
        .find(|model| model.id == "TodoView")
        .is_some_and(|model| model.record_revisions && model.tombstones));
    assert!(role_manifest
        .models
        .iter()
        .find(|model| model.id == "UserView")
        .is_some_and(|model| !model.record_revisions && !model.tombstones));
    let role_json = serde_json::to_string(&role_manifest).unwrap();
    assert!(!role_json.contains("project_user_team"));
    assert!(!role_json.contains("private-user.changed"));

    let application = surface_for_application(
        &full,
        "web",
        &["admin".into(), "restricted".into()],
        &BTreeMap::from([("admin".into(), admin), ("restricted".into(), restricted)]),
    )
    .unwrap();
    let application_manifest =
        DistributedClientSurfaceExport::from_selected("todos-service", application)
            .unwrap()
            .manifest()
            .unwrap();
    assert_eq!(
        application_manifest
            .projectors
            .iter()
            .map(|projector| projector.name.as_str())
            .collect::<Vec<_>>(),
        vec!["project_todos"]
    );
    assert!(!application_manifest.capabilities.record_revisions);
    assert!(!application_manifest.capabilities.tombstones);
    assert!(!application_manifest.capabilities.live_resume);
    let application_json = serde_json::to_string(&application_manifest).unwrap();
    assert!(!application_json.contains("project_user_team"));
    assert!(!application_json.contains("private-user.changed"));
}

#[test]
fn aggregate_nodes_export_exact_role_bounded_window_semantics() {
    let mut options = SurfaceOptions::sqlite();
    options.default_limit = 7;
    options.max_limit = 19;
    let full = build_surface(&[teams(), users(), team_members()], &options).unwrap();
    let selected = surface_for_role(
        &full,
        "admin",
        &BTreeMap::from([
            ("TeamView".into(), RoleGrant::all_columns()),
            (
                "UserView".into(),
                RoleGrant::all_columns().with_aggregations().limit(13),
            ),
        ]),
    )
    .unwrap();
    let manifest = client_manifest_from_surface(
        "teams-service",
        ClientSurfaceIdentity::role("admin"),
        &selected,
    )
    .unwrap();

    let users_aggregate = manifest
        .roots
        .iter()
        .find(|root| root.operation == ClientRootOperation::Query && root.name == "users_aggregate")
        .expect("aggregate root grant");
    assert!(
        users_aggregate.pagination.is_none(),
        "ordinary root pagination must not stand in for aggregate nodes"
    );
    let root_semantics = users_aggregate
        .aggregate
        .as_ref()
        .expect("aggregate root semantics");
    assert_eq!(root_semantics.wrapper_typename, "users_aggregate");
    assert_eq!(root_semantics.fields_typename, "users_aggregate_fields");
    assert_eq!(
        root_semantics.nodes_pagination,
        ClientPaginationSemantics {
            kind: "offset".into(),
            default_limit: 7,
            max_limit: 13,
            coverage: "window".into(),
        }
    );

    let members_aggregate = manifest
        .models
        .iter()
        .find(|model| model.id == "TeamView")
        .unwrap()
        .relationships
        .iter()
        .find(|relationship| relationship.name == "members")
        .unwrap()
        .aggregate
        .as_ref()
        .expect("relationship aggregate grant");
    assert_eq!(
        members_aggregate.semantics.wrapper_typename,
        "users_aggregate"
    );
    assert_eq!(
        members_aggregate.semantics.fields_typename,
        "users_aggregate_fields"
    );
    assert_eq!(
        members_aggregate.semantics.nodes_pagination,
        root_semantics.nodes_pagination
    );
    assert_eq!(
        serde_json::to_value(&members_aggregate.semantics).unwrap()["nodes_pagination"],
        serde_json::json!({
            "kind": "offset",
            "default_limit": 7,
            "max_limit": 13,
            "coverage": "window"
        })
    );
}

#[test]
fn opaque_m2m_plan_preserves_invalidation_without_leaking_join_internals() {
    let full = build_surface(
        &[teams(), users(), team_members()],
        &SurfaceOptions::sqlite(),
    )
    .unwrap();
    let admin = surface_for_role(
        &full,
        "admin",
        &BTreeMap::from([
            ("TeamView".into(), RoleGrant::all_columns()),
            (
                "UserView".into(),
                RoleGrant::all_columns().with_aggregations(),
            ),
        ]),
    )
    .unwrap();
    let manifest = client_manifest_from_surface(
        "teams-service",
        ClientSurfaceIdentity::role("admin"),
        &admin,
    )
    .unwrap();
    assert!(!manifest.capabilities.causal_receipts);
    assert!(manifest.capabilities.cache_scope);
    assert!(manifest.protocol_operations.command_status.is_none());
    let members = manifest
        .models
        .iter()
        .find(|model| model.id == "TeamView")
        .unwrap()
        .relationships
        .iter()
        .find(|relationship| relationship.name == "members")
        .unwrap();
    assert!(!members.nullable, "list relationships are non-null lists");
    let RelationshipKeyMapping::ThroughOpaque {
        local,
        remote,
        dependency,
    } = &members.key_mapping
    else {
        panic!("authorized source/target keys should retain an opaque m2m plan")
    };
    assert_eq!(local, &["team_id"]);
    assert_eq!(remote, &["user_id"]);
    assert!(dependency.starts_with("opaque:sha256:"));
    assert_eq!(
        members.maintenance,
        ClientRelationshipMaintenance::Revalidate
    );
    let aggregate = members.aggregate.as_ref().expect("aggregate grant");
    assert_eq!(aggregate.name, "members_aggregate");
    assert_eq!(aggregate.semantics.wrapper_typename, "users_aggregate");
    assert_eq!(
        aggregate.semantics.fields_typename,
        "users_aggregate_fields"
    );
    assert!(aggregate.semantics.count && aggregate.semantics.nodes);
    assert_eq!(aggregate.dependencies, members.dependencies);
    let users_aggregate = manifest
        .roots
        .iter()
        .find(|root| root.operation == ClientRootOperation::Query && root.name == "users_aggregate")
        .expect("aggregate root grant");
    let users_aggregate_semantics = users_aggregate
        .aggregate
        .as_ref()
        .expect("aggregate root semantics");
    assert_eq!(
        users_aggregate_semantics.wrapper_typename,
        "users_aggregate"
    );
    assert_eq!(
        users_aggregate_semantics.fields_typename,
        "users_aggregate_fields"
    );
    assert!(members.dependencies.contains(&"teams".into()));
    assert!(members.dependencies.contains(&"users".into()));
    assert!(members
        .dependencies
        .iter()
        .any(|dependency| dependency.starts_with("opaque:sha256:")));
    let json = serde_json::to_string(&manifest).unwrap();
    assert!(!json.contains("private_team_members"));

    let mut renamed_team = teams();
    renamed_team.relationships[0].through = Some("renamed_private_join".into());
    let mut renamed_join = team_members();
    renamed_join.table_name = "renamed_private_join".into();
    let renamed_full = build_surface(
        &[renamed_team, users(), renamed_join],
        &SurfaceOptions::sqlite(),
    )
    .unwrap();
    let renamed_admin = surface_for_role(
        &renamed_full,
        "admin",
        &BTreeMap::from([
            ("TeamView".into(), RoleGrant::all_columns()),
            (
                "UserView".into(),
                RoleGrant::all_columns().with_aggregations(),
            ),
        ]),
    )
    .unwrap();
    let renamed_manifest = client_manifest_from_surface(
        "teams-service",
        ClientSurfaceIdentity::role("admin"),
        &renamed_admin,
    )
    .unwrap();
    let renamed_members = renamed_manifest
        .models
        .iter()
        .find(|model| model.id == "TeamView")
        .unwrap()
        .relationships
        .iter()
        .find(|relationship| relationship.name == "members")
        .unwrap();
    let RelationshipKeyMapping::ThroughOpaque {
        dependency: renamed_dependency,
        ..
    } = &renamed_members.key_mapping
    else {
        panic!("renamed private join should remain opaque")
    };
    assert_eq!(renamed_dependency, dependency);

    let denied = surface_for_role(
        &full,
        "limited",
        &BTreeMap::from([
            ("TeamView".into(), RoleGrant::columns(["name"])),
            ("UserView".into(), RoleGrant::all_columns()),
        ]),
    )
    .unwrap();
    let denied = client_manifest_from_surface(
        "teams-service",
        ClientSurfaceIdentity::role("limited"),
        &denied,
    )
    .unwrap();
    let team = denied
        .models
        .iter()
        .find(|model| model.id == "TeamView")
        .unwrap();
    let members = team
        .relationships
        .iter()
        .find(|relationship| relationship.name == "members")
        .unwrap();
    assert_eq!(members.key_mapping, RelationshipKeyMapping::Embedded);
    assert_eq!(
        members.maintenance,
        ClientRelationshipMaintenance::Revalidate
    );
    assert!(!members.dependencies.is_empty());
    assert!(members.aggregate.is_none());
    let team_json = serde_json::to_string(team).unwrap();
    assert!(!team_json.contains("team_id"));
    assert!(!team_json.contains("private_team_members"));
}

#[test]
fn visible_read_model_join_emits_explicit_local_m2m_plan() {
    let mut join = team_members();
    join.model_name = "TeamMemberView".into();
    join.kind = TableKind::ReadModel;
    let full = build_surface(&[teams(), users(), join], &SurfaceOptions::sqlite()).unwrap();
    let selected = surface_for_role(
        &full,
        "admin",
        &BTreeMap::from([
            ("TeamView".into(), RoleGrant::all_columns()),
            ("UserView".into(), RoleGrant::all_columns()),
            ("TeamMemberView".into(), RoleGrant::all_columns()),
        ]),
    )
    .unwrap();
    let manifest = DistributedClientSurfaceExport::from_selected("teams-service", selected)
        .unwrap()
        .manifest()
        .unwrap();
    let members = manifest
        .models
        .iter()
        .find(|model| model.id == "TeamView")
        .unwrap()
        .relationships
        .iter()
        .find(|relationship| relationship.name == "members")
        .unwrap();
    assert_eq!(
        members.key_mapping,
        RelationshipKeyMapping::Through {
            local: vec!["team_id".into()],
            remote: vec!["user_id".into()],
            table: "private_team_members".into(),
            source_foreign_key: "team_id".into(),
            target_foreign_key: "user_id".into(),
        }
    );
    assert_eq!(members.maintenance, ClientRelationshipMaintenance::Local);
    assert_eq!(
        members.dependencies,
        vec!["private_team_members", "teams", "users"]
    );
}

#[test]
fn relational_row_policy_is_server_only_when_relationship_key_is_hidden() {
    let full = build_surface(&[todos(), users()], &SurfaceOptions::sqlite()).unwrap();
    let selected = surface_for_role(
        &full,
        "user",
        &BTreeMap::from([
            (
                "TodoView".into(),
                RoleGrant::columns(["todo_id", "title", "completed"])
                    .rows(rel("owner", col("display_name").eq("Patrick"))),
            ),
            ("UserView".into(), RoleGrant::columns(["display_name"])),
        ]),
    )
    .unwrap();
    let manifest = client_manifest_from_surface(
        "todos-service",
        ClientSurfaceIdentity::role("user"),
        &selected,
    )
    .unwrap();
    let todo = manifest
        .models
        .iter()
        .find(|model| model.id == "TodoView")
        .unwrap();
    assert_eq!(todo.row_policy, ClientRowPolicy::ServerOnly);
    let owner = todo
        .relationships
        .iter()
        .find(|relationship| relationship.name == "owner")
        .unwrap();
    assert_eq!(owner.key_mapping, RelationshipKeyMapping::Embedded);
    assert_eq!(owner.dependencies, vec!["todos", "users"]);
    assert_eq!(owner.maintenance, ClientRelationshipMaintenance::Revalidate);
    let json = serde_json::to_string(todo).unwrap();
    assert!(!json.contains("owner_id"));
    assert!(!json.contains("user_id"));
    assert!(!json.contains("Patrick"));
}

#[test]
fn application_role_sets_are_canonical_before_fingerprinting() {
    let full = full_surface();
    let selected =
        surface_for_application(&full, "web", &["admin".into(), "user".into()], &grants()).unwrap();
    let first = client_manifest_from_surface(
        "todos-service",
        ClientSurfaceIdentity::Application {
            name: "web".into(),
            roles: vec!["user".into(), "admin".into(), "user".into()],
        },
        &selected,
    )
    .unwrap();
    let second = client_manifest_from_surface(
        "todos-service",
        ClientSurfaceIdentity::Application {
            name: "web".into(),
            roles: vec!["admin".into(), "user".into()],
        },
        &selected,
    )
    .unwrap();
    assert_eq!(first, second);
    assert_eq!(first.schema_fingerprint, second.schema_fingerprint);
    assert_eq!(
        first.surface,
        ClientSurfaceIdentity::application("web", ["admin", "user"])
    );
}

#[test]
fn catalog_or_mismatched_surface_cannot_be_labeled_as_authorized() {
    let full = full_surface();
    let error =
        DistributedClientSurfaceExport::from_selected("todos-service", full.clone()).unwrap_err();
    assert!(error
        .to_string()
        .contains("explicitly role- or application-selected"));

    let selected = surface_for_role(&full, "user", &grants()["user"]).unwrap();
    let wrong_project = DistributedProjectManifest::new("wrong-service").table_schema(users());
    let inventory_error =
        DistributedClientSurfaceExport::from_project(&wrong_project, selected.clone()).unwrap_err();
    assert!(inventory_error.to_string().contains("does not match"));

    let error = client_manifest_from_surface(
        "todos-service",
        ClientSurfaceIdentity::role("admin"),
        &selected,
    )
    .unwrap_err();
    assert!(error.to_string().contains("does not match"));
}
