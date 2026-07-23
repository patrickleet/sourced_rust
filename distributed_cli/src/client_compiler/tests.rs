use serde_json::{json, Value as JsonValue};
use sha2::{Digest, Sha256};

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
                "dependencies": [],
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

fn list_root(operation: &str) -> JsonValue {
    json!({
        "id": format!("{operation}:todos"),
        "operation": operation,
        "name": "todos",
        "kind": "list",
        "model": "Todo",
        "arguments": list_arguments(),
        "filter": null,
        "order": null,
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
    json!({
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
    })
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
fn rejects_surface_relationship_fragment_conditional_and_multi_root() {
    let cases = [
        (
            "query Wrong { todos { owner { id } } }",
            "client.selection.relationship_unsupported",
        ),
        (
            "query Fragmented { todos { ...Fields } } fragment Fields on todo { id }",
            "client.graphql.fragments_unsupported",
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
    subscription["dependencies"] = json!(["different_table"]);
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
    let mutation = "mutation Client_createTodo($commandId: ID!, $input: JSON!) { createTodo(commandId: $commandId, input: $input) }";
    let status =
        "query Distributed_CommandStatus($commandId: ID!) { commandStatus(commandId: $commandId) { state } }";
    let mut value = manifest();
    value["capabilities"]["causal_receipts"] = json!(true);
    value["commands"] = json!([{
        "version": 1,
        "name": "CreateTodo",
        "mutation_field": "createTodo",
        "grants": ["user"],
        "input": {"kind": "json", "codec": "json"},
        "output": {"kind": "json", "codec": "json"},
        "operation": mutation,
        "operation_hash": fingerprint(mutation),
        "extensions": {
            "version": 2,
            "consistency": {"version": 1, "kind": "projected"},
            "input_defaults": {
                "version": 1,
                "defaults": [{"path": ["id"], "generator": "uuid_v7"}]
            },
            "effects": {
                "version": 1,
                "operations": [{"kind": "upsert", "model": "Todo"}],
                "fallback": "revalidate"
            },
            "confirmations": {
                "version": 1,
                "kind": "finite",
                "expected": [{"projector": "todos"}],
                "fallback": "revalidate"
            }
        }
    }]);
    value["protocol_operations"] = json!({
        "version": 1,
        "command_status": {
            "name": "Distributed_CommandStatus",
            "operation": status,
            "operation_hash": fingerprint(status)
        }
    });
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
fn rejects_commands_without_causal_identity_or_normative_input_defaults() {
    let valid = "mutation Client_createTodo($commandId: ID!, $input: JSON!) { createTodo(commandId: $commandId, input: $input) }";
    let status = "query Distributed_CommandStatus($commandId: ID!) { commandStatus(commandId: $commandId) { state } }";
    let command = |operation: &str, generator: &str| {
        json!({
            "version": 1,
            "name": "CreateTodo",
            "mutation_field": "createTodo",
            "grants": ["user"],
            "input": {"kind": "json", "codec": "json"},
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

    let without_id = "mutation Client_createTodo($input: JSON!) { createTodo(input: $input) }";
    assert_eq!(
        compile(command(without_id, "uuid_v7"))
            .expect_err("missing causal identity")
            .code,
        "client.manifest.operation_command_id"
    );
    assert_eq!(
        compile(command(valid, "uuid_v4"))
            .expect_err("unsupported generator")
            .code,
        "client.manifest.input_default_generator"
    );
}

#[test]
fn output_is_identical_for_shuffled_set_like_manifest_and_documents() {
    let mut left_manifest = manifest();
    left_manifest["models"]
        .as_array_mut()
        .expect("models")
        .push(model("Unused", "unused"));
    let mut right_manifest = left_manifest.clone();
    for key in ["models", "roots", "scalar_codecs", "projectors", "commands"] {
        right_manifest[key]
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
