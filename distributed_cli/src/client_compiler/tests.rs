use serde_json::{json, Value as JsonValue};
use sha2::{Digest, Sha256};

use super::manifest::{refresh_schema_fingerprint, ClientManifest};
use super::{
    compile_client, ClientCompileInput, ClientDocument, ClientRouteDiscovery,
    ClientRouteRegistration, ClientSurfaceSelector,
};

fn fingerprint(label: &str) -> String {
    let digest = Sha256::digest(label.as_bytes());
    format!("sha256:{digest:x}")
}

fn scalar_codecs() -> JsonValue {
    json!([
        {"scalar": "BigInt", "codec": "json_number_precision_limited"},
        {"scalar": "Boolean", "codec": "boolean"},
        {"scalar": "Bytea", "codec": "base64"},
        {"scalar": "Float", "codec": "float64"},
        {"scalar": "ID", "codec": "string"},
        {"scalar": "Int", "codec": "int32"},
        {"scalar": "JSON", "codec": "json"},
        {"scalar": "String", "codec": "string"},
        {"scalar": "Timestamptz", "codec": "string_unvalidated_timestamp"}
    ])
}

fn model(id: &str, typename: &str) -> JsonValue {
    json!({
        "id": id,
        "typename": typename,
        "source_table": format!("{}_rows", id.to_lowercase()),
        "dependencies": [format!("{}_rows", id.to_lowercase())],
        "normalization": {
            "kind": "normalized",
            "fields": [
                {"name": "tenantId", "codec": "string"},
                {"name": "id", "codec": "string"}
            ],
            "encoding": "canonical_json_tuple_v1"
        },
        "fields": [
            {"name": "completed", "scalar": "Boolean", "codec": "boolean", "nullable": false},
            {"name": "id", "scalar": "ID", "codec": "string", "nullable": false},
            {"name": "priority", "scalar": "Int", "codec": "int32", "nullable": false},
            {"name": "tenantId", "scalar": "ID", "codec": "string", "nullable": false},
            {"name": "title", "scalar": "String", "codec": "string", "nullable": true}
        ],
        "relationships": [
            {
                "name": "owner",
                "target_model": id,
                "target_typename": typename,
                "kind": "belongs_to",
                "list": false,
                "nullable": true,
                "arguments": [],
                "key_mapping": {"kind": "embedded"},
                "maintenance": "revalidate",
                "dependencies": [format!("{}_rows", id.to_lowercase())],
                "live": false
            }
        ],
        "row_policy": {"kind": "unrestricted"},
        "record_revisions": true,
        "tombstones": true
    })
}

fn list_arguments() -> JsonValue {
    json!([
        {
            "name": "where",
            "kind": "filter",
            "type_name": "todo_bool_exp",
            "nullable": true,
            "list": false
        },
        {
            "name": "order_by",
            "kind": "order",
            "type_name": "todo_order_by",
            "nullable": true,
            "list": true
        },
        {
            "name": "limit",
            "kind": "limit",
            "type_name": "Int",
            "nullable": true,
            "list": false,
            "codec": "int32"
        },
        {
            "name": "offset",
            "kind": "offset",
            "type_name": "Int",
            "nullable": true,
            "list": false,
            "codec": "int32"
        }
    ])
}

fn filter_semantics() -> JsonValue {
    json!({
        "fields": [
            {"name": "completed", "operators": ["_eq"]},
            {"name": "id", "operators": ["_eq"]},
            {"name": "priority", "operators": ["_eq"]},
            {"name": "tenantId", "operators": ["_eq"]},
            {"name": "title", "operators": ["_eq"]}
        ],
        "relationships": ["owner"],
        "row_policy": {"kind": "unrestricted"}
    })
}

fn order_semantics() -> JsonValue {
    json!({
        "fields": ["completed", "id", "priority", "tenantId", "title"],
        "values": [
            "asc",
            "asc_nulls_first",
            "asc_nulls_last",
            "desc",
            "desc_nulls_first",
            "desc_nulls_last"
        ]
    })
}

fn list_root(operation: &str) -> JsonValue {
    json!({
        "id": format!("{operation}:todos"),
        "operation": operation,
        "name": "todos",
        "kind": "list",
        "model": "Todo",
        "arguments": list_arguments(),
        "filter": filter_semantics(),
        "order": order_semantics(),
        "pagination": {
            "kind": "offset",
            "default_limit": 25,
            "max_limit": 100,
            "coverage": "window"
        },
        "aggregate": null,
        "dependencies": ["todo_rows"],
        "live": true
    })
}

fn by_pk_root() -> JsonValue {
    json!({
        "id": "query:todo",
        "operation": "query",
        "name": "todo",
        "kind": "by_pk",
        "model": "Todo",
        "arguments": [
            {
                "name": "tenantId",
                "kind": "primary_key",
                "type_name": "ID",
                "nullable": false,
                "list": false,
                "codec": "string"
            },
            {
                "name": "id",
                "kind": "primary_key",
                "type_name": "ID",
                "nullable": false,
                "list": false,
                "codec": "string"
            }
        ],
        "filter": null,
        "order": null,
        "pagination": null,
        "aggregate": null,
        "dependencies": ["todo_rows"],
        "live": false
    })
}

fn manifest() -> JsonValue {
    let mut value = json!({
        "manifest_version": 4,
        "protocol_version": 2,
        "service_id": "todos-service",
        "surface": {"kind": "role", "name": "user"},
        "schema_fingerprint": fingerprint("schema"),
        "protocol_fingerprint": "sha256:50a3690689ff5aa7cefc88bb7b5d6f1e1a64615e7644d306403287c09b1e59dc",
        "capabilities": {
            "live_queries": true,
            "record_revisions": true,
            "tombstones": true,
            "causal_receipts": false,
            "live_resume": true,
            "query_fallback": "revalidate",
            "cache_scope": false,
            "confirmed_persistence": false
        },
        "scalar_codecs": scalar_codecs(),
        "models": [model("Todo", "todo")],
        "roots": [list_root("query"), list_root("subscription"), by_pk_root()],
        "commands": [],
        "protocol_operations": {"version": 1},
        "projectors": []
    });
    refresh_schema_fingerprint(&mut value);
    value
}

fn projected_manifest() -> JsonValue {
    let mutation = "mutation Client_projectTodo($commandId: ID!, $input: ProjectTodoInput!) { projectTodo(commandId: $commandId, input: $input) { completed id priority tenantId title } }";
    let status =
        "query Distributed_CommandStatus($commandId: ID!) { commandStatus(commandId: $commandId) { state } }";
    let mut value = manifest();
    value["capabilities"]["causal_receipts"] = json!(true);
    value["capabilities"]["cache_scope"] = json!(true);
    value["commands"] = json!([{
        "version": 1,
        "name": "ProjectTodo",
        "mutation_field": "projectTodo",
        "grants": ["user"],
        "input": {
            "kind": "object",
            "definition": {
                "name": "ProjectTodoInput",
                "fields": [
                    {
                        "name": "id",
                        "type_name": "ID",
                        "nullable": false,
                        "list": false,
                        "item_nullable": false,
                        "codec": "string"
                    },
                    {
                        "name": "tenantId",
                        "type_name": "ID",
                        "nullable": false,
                        "list": false,
                        "item_nullable": false,
                        "codec": "string"
                    }
                ]
            }
        },
        "output": {
            "kind": "object",
            "definition": {
                "name": "todo",
                "fields": [
                    {
                        "name": "completed",
                        "type_name": "Boolean",
                        "nullable": false,
                        "list": false,
                        "item_nullable": false,
                        "codec": "boolean"
                    },
                    {
                        "name": "id",
                        "type_name": "ID",
                        "nullable": false,
                        "list": false,
                        "item_nullable": false,
                        "codec": "string"
                    },
                    {
                        "name": "priority",
                        "type_name": "Int",
                        "nullable": false,
                        "list": false,
                        "item_nullable": false,
                        "codec": "int32"
                    },
                    {
                        "name": "tenantId",
                        "type_name": "ID",
                        "nullable": false,
                        "list": false,
                        "item_nullable": false,
                        "codec": "string"
                    },
                    {
                        "name": "title",
                        "type_name": "String",
                        "nullable": true,
                        "list": false,
                        "item_nullable": false,
                        "codec": "string"
                    }
                ]
            }
        },
        "operation": mutation,
        "operation_hash": fingerprint(mutation),
        "extensions": {
            "version": 2,
            "consistency": {"version": 1, "kind": "projected"},
            "direct_projection": {
                "topology": {
                    "version": 1,
                    "name": "todos",
                    "digest": fingerprint("todos topology")
                },
                "model": "Todo",
                "partition": {"kind": "input", "path": ["tenantId"]},
                "change_epoch": "todos-v1"
            }
        }
    }]);
    value["projectors"] = json!([{
        "version": 1,
        "name": "todos",
        "facts": ["TodoProjected"],
        "models": ["Todo"],
        "dependencies": ["todo_rows"],
        "causal_confirmation": false
    }]);
    value["protocol_operations"] = json!({
        "version": 1,
        "command_status": {
            "name": "Distributed_CommandStatus",
            "operation": status,
            "operation_hash": fingerprint(status)
        }
    });
    refresh_schema_fingerprint(&mut value);
    value
}

fn input(source: &str) -> ClientCompileInput {
    ClientCompileInput::new(
        manifest(),
        ClientSurfaceSelector::role("user"),
        vec![ClientDocument::new(
            "src/routes/todos/+page.graphql",
            source,
        )],
    )
}

fn file<'a>(project: &'a super::GeneratedClientProject, path: &str) -> &'a str {
    project
        .files
        .iter()
        .find(|file| file.path == path)
        .unwrap_or_else(|| panic!("missing generated file {path}"))
        .contents
        .as_str()
}

#[test]
fn compiles_aliases_composite_wire_identity_live_and_load() {
    let project = compile_client(input(
        r#"
          query Todos($limit: Int!) @load @live {
            rows: todos(limit: $limit) {
              _distributed_tenantId: title
              headline: title
            }
          }
        "#,
    ))
    .expect("compile");
    let operation = &project.operations[0];
    let generated = file(&project, &operation.module_path);

    assert!(generated.contains("query Todos($limit: Int!)"));
    assert!(generated.contains("_distributed_tenantId_2: tenantId"));
    assert!(generated.contains("_distributed_id: id"));
    assert!(generated.contains("_distributed_typename: __typename"));
    assert!(generated.contains("subscription Todos_Live($limit: Int!)"));
    assert!(generated.contains("\"expose\": false"));
    assert!(generated.contains("readonly \"headline\": string | null"));
    assert!(!generated.contains("readonly \"_distributed_id\""));
    assert_eq!(
        operation.operation_hash,
        fingerprint(
            "query Todos($limit: Int!) {\n  rows: todos(limit: $limit) {\n    _distributed_tenantId: title\n    headline: title\n    _distributed_tenantId_2: tenantId\n    _distributed_id: id\n    _distributed_typename: __typename\n  }\n}\n"
        )
    );
    assert!(operation.live_operation_hash.is_some());
    assert_eq!(project.routes[0].route, "/todos");
    assert_eq!(
        project.routes[0].discovery,
        ClientRouteDiscovery::Convention
    );
}

#[test]
fn preserves_enum_wire_syntax_and_json_runtime_value() {
    let project = compile_client(input(
        r#"
          query Ordered {
            todos(order_by: [{priority: asc}]) { id }
          }
        "#,
    ))
    .expect("compile");
    let generated = file(&project, &project.operations[0].module_path);
    assert!(generated.contains("todos(order_by: [{priority: asc}])"));
    assert!(generated.contains("\"priority\": \"asc\""));
    assert!(!generated.contains("priority: \\\"asc\\\""));
}

#[test]
fn variable_nullability_is_compatible_but_never_weaker() {
    compile_client(input(
        "query Strict($limit: Int!) { todos(limit: $limit) { id } }",
    ))
    .expect("non-null variable may satisfy nullable argument");

    let error = compile_client(input(
        "query Weak($tenantId: ID, $id: ID!) { todo(tenantId: $tenantId, id: $id) { title } }",
    ))
    .expect_err("nullable variable may not satisfy non-null argument");
    assert_eq!(error.code, "client.variable.type_mismatch");

    let error = compile_client(input(
        "query WeakItem($order: [todo_order_by]) { todos(order_by: $order) { id } }",
    ))
    .expect_err("nullable list item may not satisfy non-null item");
    assert_eq!(error.code, "client.variable.type_mismatch");
}

#[test]
fn rejects_variable_defaults_until_cache_identity_can_apply_them() {
    let error = compile_client(input(
        "query Defaulted($limit: Int = 10) { todos(limit: $limit) { id } }",
    ))
    .expect_err("defaults would currently diverge from replica argument identity");
    assert_eq!(error.code, "client.variable.default_unsupported");
}

#[test]
fn explicit_load_registration_is_the_documented_fallback() {
    let input = ClientCompileInput::new(
        manifest(),
        ClientSurfaceSelector::role("user"),
        vec![ClientDocument::new(
            "src/features/todos.graphql",
            "query Todos @load { todos { id } }",
        )],
    );
    let error = compile_client(input.clone()).expect_err("registration required");
    assert_eq!(error.code, "client.route.registration_required");
    assert!(error.message.contains("--route Todos=/route-id"));

    let project = compile_client(
        input.with_route_registrations(vec![ClientRouteRegistration::new("Todos", "/todos")]),
    )
    .expect("explicit route");
    assert_eq!(project.routes[0].route, "/todos");
    assert_eq!(project.routes[0].discovery, ClientRouteDiscovery::Explicit);
}

#[test]
fn expands_root_named_and_inline_fragments_into_canonical_full_text() {
    let project = compile_client(input(
        r#"
          query Fragmented {
            ...Root
          }
          fragment Root on Query {
            todos {
              ...Fields
              ... { priority }
              ... on todo { headline: title }
              ...Fields
            }
          }
          fragment Fields on todo { id }
        "#,
    ))
    .expect("compile fragments");
    let expected = "query Fragmented {\n  todos {\n    id\n    priority\n    headline: title\n    _distributed_tenantId: tenantId\n    _distributed_typename: __typename\n  }\n}\n";

    assert_eq!(project.operations[0].operation_hash, fingerprint(expected));
    assert!(!file(&project, &project.operations[0].module_path).contains("fragment Root"));
}

#[test]
fn merges_identical_root_objects_and_preserves_first_encounter_order() {
    let project = compile_client(input(
        r#"
          query Merged {
            ...First
            ...Second
          }
          fragment First on Query {
            rows: todos(limit: 10, offset: 0) { title }
          }
          fragment Second on Query {
            rows: todos(offset: 0, limit: 10) { id title }
          }
        "#,
    ))
    .expect("merge compatible root selections");
    let expected = "query Merged {\n  rows: todos(limit: 10, offset: 0) {\n    title\n    id\n    _distributed_tenantId: tenantId\n    _distributed_typename: __typename\n  }\n}\n";

    assert_eq!(project.operations[0].operation_hash, fingerprint(expected));
}

#[test]
fn rejects_fragment_graph_errors_deterministically() {
    let cases = [
        (
            "query Missing { todos { ...Absent } }",
            "client.graphql.fragment_undefined",
            "Absent",
        ),
        (
            "query Cyclic { todos { ...A } } fragment A on todo { ...B } fragment B on todo { ...A }",
            "client.graphql.fragment_cycle",
            "A -> B -> A",
        ),
        (
            "query WrongType { todos { ...Fields } } fragment Fields on Todo { id }",
            "client.graphql.fragment_type",
            "current concrete type is `todo`",
        ),
        (
            "query WrongRoot { ...Root } fragment Root on query { todos { id } }",
            "client.graphql.fragment_type",
            "current concrete type is `Query`",
        ),
        (
            "query WrongInline { todos { ... on Todo { id } } }",
            "client.graphql.fragment_type",
            "current concrete type is `todo`",
        ),
    ];
    for (source, code, message) in cases {
        let error = compile_client(input(source)).expect_err(source);
        assert_eq!(error.code, code, "{source}: {error}");
        assert!(error.message.contains(message), "{source}: {error}");
        assert!(error.source.is_some());
    }

    let error = compile_client(input(
        r#"
          query Unused { todos { id } }
          fragment ZedFirst on todo { id }
          fragment AlphaLater on todo { title }
        "#,
    ))
    .expect_err("source-first unused fragment");
    assert_eq!(error.code, "client.graphql.fragment_unused");
    assert!(error.message.contains("ZedFirst"));
}

#[test]
fn prevalidates_fragment_graph_through_nested_fields_before_lowering() {
    let error = compile_client(input(
        r#"
          query NestedCycle {
            todos { ...Recursive }
          }
          fragment Recursive on todo {
            owner { ...Recursive }
          }
        "#,
    ))
    .expect_err("fragment cycles remain invalid through relationship fields");
    assert_eq!(error.code, "client.graphql.fragment_cycle");
    assert!(error.message.contains("Recursive -> Recursive"));
    assert_eq!(error.source.as_ref().map(|source| source.line), Some(6));

    let error = compile_client(input(
        "query NestedMissing { todos { owner { ...Missing } } }",
    ))
    .expect_err("undefined nested fragments precede unsupported relationship lowering");
    assert_eq!(error.code, "client.graphql.fragment_undefined");
    assert!(error.message.contains("Missing"));
}

#[test]
fn fragment_definition_order_does_not_change_generated_output() {
    let operation = "query Stable { todos { ...First ...Second } }\n";
    let left = compile_client(input(&format!(
        "{operation}fragment First on todo {{ id }}\nfragment Second on todo {{ title }}\n"
    )))
    .expect("first definition order");
    let right = compile_client(input(&format!(
        "{operation}fragment Second on todo {{ title }}\nfragment First on todo {{ id }}\n"
    )))
    .expect("second definition order");

    assert_eq!(left, right);
}

#[test]
fn rejects_directives_on_every_fragment_surface() {
    let cases = [
        (
            "query SpreadDirective { todos { ...Fields @skip(if: true) } } fragment Fields on todo { id }",
            "client.directive.conditional_unsupported",
        ),
        (
            "query DefinitionDirective { todos { ...Fields } } fragment Fields on todo @custom { id }",
            "client.directive.unsupported",
        ),
        (
            "query InlineDirective { todos { ... @custom { id } } }",
            "client.directive.unsupported",
        ),
    ];
    for (source, code) in cases {
        let error = compile_client(input(source)).expect_err(source);
        assert_eq!(error.code, code, "{source}: {error}");
    }
}

#[test]
fn rejects_response_key_conflicts_at_the_later_selection() {
    let source = r#"
      query Conflict {
        todos {
          same: id
          same: title
        }
      }
    "#;
    let error = compile_client(input(source)).expect_err("field-name conflict");
    assert_eq!(error.code, "client.selection.conflict");
    assert_eq!(error.source.as_ref().map(|source| source.line), Some(5));
    assert!(error.message.contains("first selection at 4:"));

    for source in [
        "query ShapeConflict { todos { same: id same: id { value } } }",
        "query ArgumentConflict { rows: todos(limit: 1) { id } rows: todos(limit: 2) { id } }",
    ] {
        let error = compile_client(input(source)).expect_err(source);
        assert_eq!(error.code, "client.selection.conflict", "{source}: {error}");
    }
}

#[test]
fn fragment_expansion_is_bounded_by_depth_and_total_work() {
    let mut deep = String::from("query Deep { todos { ...F0 } }\n");
    for index in 0..65 {
        deep.push_str(&format!(
            "fragment F{index} on todo {{ ...F{} }}\n",
            index + 1
        ));
    }
    deep.push_str("fragment F65 on todo { id }\n");
    let error = compile_client(input(&deep)).expect_err("depth bound");
    assert_eq!(error.code, "client.selection.depth");

    let spreads = "...Fields ".repeat(5_000);
    let expanded =
        format!("query Expanded {{ todos {{ {spreads} }} }} fragment Fields on todo {{ id }}");
    let error = compile_client(input(&expanded)).expect_err("expansion work bound");
    assert_eq!(error.code, "client.selection.expansion_bound");
}

#[test]
fn rejects_surface_relationship_conditional_and_multi_root() {
    let cases = [
        (
            "query Wrong { todos { owner { id } } }",
            "client.selection.relationship_unsupported",
        ),
        (
            "query Conditional($yes: Boolean!) { todos { id @include(if: $yes) } }",
            "client.directive.conditional_unsupported",
        ),
        (
            "query Multi { todos { id } todo(tenantId: \"t\", id: \"1\") { id } }",
            "client.operation.single_root",
        ),
    ];
    for (source, code) in cases {
        let error = compile_client(input(source)).expect_err(source);
        assert_eq!(error.code, code, "{source}: {error}");
        assert!(error.source.is_some());
    }
}

#[test]
fn rejects_unknown_scalar_and_codec_instead_of_emitting_unknown() {
    let mut invalid = manifest();
    invalid["scalar_codecs"]
        .as_array_mut()
        .expect("array")
        .push(json!({"scalar": "Money", "codec": "money"}));
    let error = compile_client(ClientCompileInput::new(
        invalid,
        ClientSurfaceSelector::role("user"),
        vec![ClientDocument::new(
            "src/routes/todos/+page.graphql",
            "query Todos { todos { id } }",
        )],
    ))
    .expect_err("unknown codec");
    assert_eq!(error.code, "client.manifest.scalar_unsupported");
}

#[test]
fn rejects_well_formed_but_incompatible_protocol_fingerprint() {
    let mut invalid = manifest();
    invalid["protocol_fingerprint"] = json!(fingerprint("different protocol"));
    let error = compile_client(ClientCompileInput::new(
        invalid,
        ClientSurfaceSelector::role("user"),
        vec![ClientDocument::new(
            "src/routes/todos/+page.graphql",
            "query Todos { todos { id } }",
        )],
    ))
    .expect_err("protocol drift");
    assert_eq!(error.code, "client.manifest.protocol_fingerprint");
    assert!(error.message.contains("matching dctl version"));
}

#[test]
fn live_companion_fails_closed_on_dependency_or_pagination_drift() {
    let mut invalid = manifest();
    let subscription = invalid["roots"]
        .as_array_mut()
        .expect("roots")
        .iter_mut()
        .find(|root| root["operation"] == "subscription")
        .expect("subscription");
    subscription["pagination"]["default_limit"] = json!(24);
    refresh_schema_fingerprint(&mut invalid);
    let error = compile_client(ClientCompileInput::new(
        invalid,
        ClientSurfaceSelector::role("user"),
        vec![ClientDocument::new(
            "src/routes/todos/+page.graphql",
            "query Todos @live { todos { id } }",
        )],
    ))
    .expect_err("live drift");
    assert_eq!(error.code, "client.live.root_mismatch");
}

#[test]
fn command_protocol_and_extensions_are_preserved_exactly() {
    let mutation = "mutation Client_createTodo($commandId: ID!, $input: CreateTodoInput!) { createTodo(commandId: $commandId, input: $input) }";
    let status =
        "query Distributed_CommandStatus($commandId: ID!) { commandStatus(commandId: $commandId) { state } }";
    let mut value = manifest();
    value["capabilities"]["causal_receipts"] = json!(true);
    value["capabilities"]["cache_scope"] = json!(true);
    value["commands"] = json!([{
        "version": 1,
        "name": "CreateTodo",
        "mutation_field": "createTodo",
        "grants": ["user"],
        "input": {
            "kind": "object",
            "definition": {
                "name": "CreateTodoInput",
                "fields": [
                    {
                        "name": "id",
                        "type_name": "ID",
                        "nullable": false,
                        "list": false,
                        "item_nullable": false,
                        "codec": "string"
                    },
                    {
                        "name": "tenantId",
                        "type_name": "ID",
                        "nullable": false,
                        "list": false,
                        "item_nullable": false,
                        "codec": "string"
                    },
                    {
                        "name": "title",
                        "type_name": "String",
                        "nullable": false,
                        "list": false,
                        "item_nullable": false,
                        "codec": "string"
                    }
                ]
            }
        },
        "output": {"kind": "json", "codec": "json"},
        "operation": mutation,
        "operation_hash": fingerprint(mutation),
        "extensions": {
            "version": 2,
            "consistency": {"version": 1, "kind": "fact"},
            "input_defaults": {
                "version": 1,
                "defaults": [{"path": ["id"], "generator": "uuid_v7"}]
            },
            "effects": {
                "version": 1,
                "operations": [{
                    "kind": "upsert",
                    "model": "Todo",
                    "key": {
                        "fields": [
                            {
                                "field": "tenantId",
                                "value": {"kind": "input", "path": ["tenantId"]}
                            },
                            {
                                "field": "id",
                                "value": {"kind": "input", "path": ["id"]}
                            }
                        ]
                    },
                    "fields": [{
                        "field": "title",
                        "value": {"kind": "input", "path": ["title"]}
                    }]
                }],
                "fallback": "revalidate"
            },
            "confirmations": {
                "version": 1,
                "kind": "finite",
                "expected": [{
                    "projector": "todos",
                    "model": "Todo",
                    "key": {
                        "fields": [
                            {
                                "field": "tenantId",
                                "value": {"kind": "input", "path": ["tenantId"]}
                            },
                            {
                                "field": "id",
                                "value": {"kind": "input", "path": ["id"]}
                            }
                        ]
                    }
                }],
                "fallback": "revalidate"
            }
        }
    }]);
    value["projectors"] = json!([{
        "version": 1,
        "name": "todos",
        "facts": ["TodoCreated"],
        "models": ["Todo"],
        "dependencies": ["todo_rows"],
        "causal_confirmation": true
    }]);
    value["protocol_operations"] = json!({
        "version": 1,
        "command_status": {
            "name": "Distributed_CommandStatus",
            "operation": status,
            "operation_hash": fingerprint(status)
        }
    });
    refresh_schema_fingerprint(&mut value);
    let project = compile_client(ClientCompileInput::new(
        value,
        ClientSurfaceSelector::role("user"),
        vec![ClientDocument::new(
            "src/routes/todos/+page.graphql",
            "query Todos { todos { id } }",
        )],
    ))
    .expect("compile commands");
    let commands = file(&project, "commands.ts");
    let protocol = file(&project, "protocol.ts");
    assert!(commands.contains(mutation));
    assert!(commands.contains("\"input_defaults\""));
    assert!(commands.contains("\"effects\""));
    assert!(commands.contains("\"confirmations\""));
    assert!(protocol.contains(status));
    assert!(protocol.contains(&fingerprint(status)));
}

#[test]
fn projected_command_requires_exact_role_safe_direct_projection() {
    let value = projected_manifest();
    let parsed = ClientManifest::parse(value, &ClientSurfaceSelector::role("user"))
        .expect("valid projected direct target");
    let direct = parsed.commands[0]
        .extensions
        .direct_projection
        .as_ref()
        .expect("projected command direct target");
    assert_eq!(direct.topology.version, 1);
    assert_eq!(direct.topology.name, "todos");
    assert_eq!(direct.model, "Todo");
    assert_eq!(direct.change_epoch, "todos-v1");
    assert!(matches!(
        direct.partition,
        Some(super::manifest::ManifestEffectExpression::Input { ref path })
            if path.as_slice() == ["tenantId"]
    ));

    let mut absent = projected_manifest();
    absent["commands"][0]["extensions"]
        .as_object_mut()
        .expect("extensions")
        .remove("direct_projection");
    refresh_schema_fingerprint(&mut absent);
    let error = ClientManifest::parse(absent, &ClientSurfaceSelector::role("user"))
        .expect_err("projected direct target is mandatory");
    assert_eq!(error.code, "client.manifest.direct_projection_required");

    let mut non_projected = projected_manifest();
    non_projected["commands"][0]["extensions"]["consistency"]["kind"] = json!("accepted");
    refresh_schema_fingerprint(&mut non_projected);
    let error = ClientManifest::parse(non_projected, &ClientSurfaceSelector::role("user"))
        .expect_err("accepted commands cannot carry direct projection metadata");
    assert_eq!(error.code, "client.manifest.direct_projection_unexpected");
}

#[test]
fn projected_command_rejects_tampered_topology_and_wrong_owner() {
    let mut tampered = projected_manifest();
    tampered["commands"][0]["extensions"]["direct_projection"]["topology"]["digest"] =
        json!(format!("sha256:{}", "AB".repeat(32)));
    refresh_schema_fingerprint(&mut tampered);
    let error = ClientManifest::parse(tampered, &ClientSurfaceSelector::role("user"))
        .expect_err("topology digest must be canonical lowercase SHA-256");
    assert_eq!(error.code, "client.manifest.hash");

    let mut wrong_owner = projected_manifest();
    wrong_owner["commands"][0]["extensions"]["direct_projection"]["topology"]["name"] =
        json!("other_projector");
    refresh_schema_fingerprint(&mut wrong_owner);
    let error = ClientManifest::parse(wrong_owner, &ClientSurfaceSelector::role("user"))
        .expect_err("direct target must use the visible model owner");
    assert_eq!(error.code, "client.manifest.direct_projection_owner");

    let mut trusted_preset = projected_manifest();
    trusted_preset["commands"][0]["extensions"]["direct_projection"]["partition"] =
        json!({"kind": "trusted_preset", "name": "current_tenant"});
    refresh_schema_fingerprint(&mut trusted_preset);
    let error = ClientManifest::parse(trusted_preset, &ClientSurfaceSelector::role("user"))
        .expect_err("unresolved trusted presets cannot define a direct target");
    assert_eq!(error.code, "client.manifest.direct_projection_partition");
}

#[test]
fn rejects_commands_without_causal_identity_or_normative_input_defaults() {
    let valid = "mutation Client_createTodo($commandId: ID!, $input: CreateTodoInput!) { createTodo(commandId: $commandId, input: $input) }";
    let status = "query Distributed_CommandStatus($commandId: ID!) { commandStatus(commandId: $commandId) { state } }";
    let command = |operation: &str, generator: &str| {
        json!({
            "version": 1,
            "name": "CreateTodo",
            "mutation_field": "createTodo",
            "grants": ["user"],
            "input": {
                "kind": "object",
                "definition": {
                    "name": "CreateTodoInput",
                    "fields": [{
                        "name": "id",
                        "type_name": "ID",
                        "nullable": false,
                        "list": false,
                        "item_nullable": false,
                        "codec": "string"
                    }]
                }
            },
            "output": {"kind": "json", "codec": "json"},
            "operation": operation,
            "operation_hash": fingerprint(operation),
            "extensions": {
                "version": 2,
                "consistency": {"version": 1, "kind": "projected"},
                "input_defaults": {
                    "version": 1,
                    "defaults": [{"path": ["id"], "generator": generator}]
                }
            }
        })
    };
    let compile = |entry: JsonValue| {
        let mut value = manifest();
        value["capabilities"]["causal_receipts"] = json!(true);
        value["capabilities"]["cache_scope"] = json!(true);
        value["commands"] = json!([entry]);
        value["protocol_operations"] = json!({
            "version": 1,
            "command_status": {
                "name": "Distributed_CommandStatus",
                "operation": status,
                "operation_hash": fingerprint(status)
            }
        });
        compile_client(ClientCompileInput::new(
            value,
            ClientSurfaceSelector::role("user"),
            vec![ClientDocument::new(
                "src/routes/todos/+page.graphql",
                "query Todos { todos { id } }",
            )],
        ))
    };

    let without_id =
        "mutation Client_createTodo($input: CreateTodoInput!) { createTodo(input: $input) }";
    assert_eq!(
        compile(command(without_id, "uuid_v7"))
            .expect_err("missing causal identity")
            .code,
        "client.manifest.command_operation"
    );
    assert_eq!(
        compile(command(valid, "uuid_v4"))
            .expect_err("unsupported generator")
            .code,
        "client.manifest.input_default_generator"
    );
}

#[test]
fn output_is_identical_for_shuffled_documents() {
    let mut left_manifest = manifest();
    left_manifest["models"]
        .as_array_mut()
        .expect("models")
        .push(model("Unused", "unused"));
    refresh_schema_fingerprint(&mut left_manifest);
    let right_manifest = left_manifest.clone();
    let mut tampered_manifest = left_manifest.clone();
    for key in ["models", "roots", "scalar_codecs", "projectors", "commands"] {
        tampered_manifest[key]
            .as_array_mut()
            .expect("set-like array")
            .reverse();
    }
    let docs = vec![
        ClientDocument::new(
            "src/routes/todos/+page.graphql",
            "query Todos { todos { id } }",
        ),
        ClientDocument::new(
            "src/routes/todo/+page.graphql",
            "query Todo { todo(tenantId: \"t\", id: \"1\") { title } }",
        ),
    ];
    let left = compile_client(ClientCompileInput::new(
        left_manifest,
        ClientSurfaceSelector::role("user"),
        docs.clone(),
    ))
    .expect("left");
    let right = compile_client(ClientCompileInput::new(
        right_manifest,
        ClientSurfaceSelector::role("user"),
        docs.into_iter().rev().collect(),
    ))
    .expect("right");
    assert_eq!(left, right);

    let error = compile_client(ClientCompileInput::new(
        tampered_manifest,
        ClientSurfaceSelector::role("user"),
        vec![ClientDocument::new(
            "src/routes/todos/+page.graphql",
            "query Todos { todos { id } }",
        )],
    ))
    .expect_err("manifest order is part of the emitter-owned fingerprint");
    assert_eq!(error.code, "client.manifest.schema_fingerprint");
}

#[test]
fn manifest_parser_accepts_embedded_by_pk_and_zero_sized_windows() {
    let mut value = manifest();
    value["models"][0]["normalization"] = json!({"kind": "embedded"});
    value["roots"]
        .as_array_mut()
        .expect("roots")
        .iter_mut()
        .find(|root| root["kind"] == "by_pk")
        .expect("by-pk root")["arguments"] = json!([]);
    for root in value["roots"].as_array_mut().expect("roots") {
        if root["kind"] == "list" {
            root["pagination"]["default_limit"] = json!(0);
            root["pagination"]["max_limit"] = json!(0);
        }
    }
    refresh_schema_fingerprint(&mut value);

    ClientManifest::parse(value, &ClientSurfaceSelector::role("user"))
        .expect("embedded models and zero-sized authorized windows are valid v4 contracts");
}

#[test]
fn manifest_parser_accepts_mixed_per_model_revision_evidence() {
    let mut value = manifest();
    let mut without_evidence = model("Unowned", "Unowned");
    without_evidence["record_revisions"] = json!(false);
    without_evidence["tombstones"] = json!(false);
    value["models"]
        .as_array_mut()
        .expect("models")
        .push(without_evidence);
    value["capabilities"]["record_revisions"] = json!(false);
    value["capabilities"]["tombstones"] = json!(false);
    refresh_schema_fingerprint(&mut value);

    ClientManifest::parse(value, &ClientSurfaceSelector::role("user"))
        .expect("global record evidence describes the selected query footprint, not any model");
}

#[test]
fn rejects_unknown_and_duplicate_route_registrations() {
    let error = compile_client(
        input("query Todos { todos { id } }")
            .with_route_registrations(vec![ClientRouteRegistration::new("Missing", "/missing")]),
    )
    .expect_err("unknown registration");
    assert_eq!(error.code, "client.route.unknown_registration");

    let error = compile_client(
        input("query Todos { todos { id } }").with_route_registrations(vec![
            ClientRouteRegistration::new("Todos", "/one"),
            ClientRouteRegistration::new("Todos", "/two"),
        ]),
    )
    .expect_err("duplicate registration");
    assert_eq!(error.code, "client.route.duplicate_registration");
}

#[test]
fn source_paths_cannot_inject_generated_typescript() {
    let project = compile_client(ClientCompileInput::new(
        manifest(),
        ClientSurfaceSelector::role("user"),
        vec![ClientDocument::new(
            "src/injected*/\nexport const compromised = true;\n/**.graphql",
            "query Safe { todos { id } }",
        )],
    ))
    .expect("compile adversarial source path");
    let module = file(&project, &project.operations[0].module_path);

    assert!(module.starts_with("/** GENERATED by dctl client. Do not edit. */\n"));
    assert!(!module.contains("compromised"));
    assert!(file(&project, "manifest.json").contains("\\nexport const compromised"));
}
