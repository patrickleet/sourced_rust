#![allow(dead_code)]

use distributed::graphql::SurfaceProjector;
use distributed::{
    command_confirmations, command_effects, command_input_defaults, GraphqlInput, ReadModel,
    RelationalReadModel,
};
use serde::{Deserialize, Serialize};

#[derive(GraphqlInput)]
struct UpdateTodo {
    tenant_id: String,
    todo_id: String,
    title: String,
    optional_title: Option<String>,
    generated_id: String,
}

#[derive(GraphqlInput)]
struct TodoDetails {
    title: String,
}

#[derive(GraphqlInput)]
struct NestedUpdateTodo {
    tenant_id: String,
    todo_id: String,
    details: TodoDetails,
    optional_details: Option<TodoDetails>,
    generated_id: String,
}

#[derive(Clone, Serialize, Deserialize)]
enum TodoStatus {
    Open,
    Closed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
enum TextKey {
    Primary,
}

#[derive(Clone, ReadModel)]
#[readmodel(table = "text_key_views", primary_key = ["key"])]
struct TextKeyView {
    #[readmodel(text)]
    key: TextKey,
}

#[derive(Clone, Serialize, Deserialize)]
enum StructuredTextKey {
    Payload { value: String },
}

#[derive(Clone, ReadModel)]
#[readmodel(table = "structured_text_key_views", primary_key = ["key"])]
struct StructuredTextKeyView {
    #[readmodel(text)]
    key: StructuredTextKey,
}

#[derive(GraphqlInput)]
struct LinkTodo {
    parent_id: String,
    child_id: String,
}

#[derive(Clone, ReadModel)]
#[readmodel(table = "child_views", primary_key = ["child_id"])]
struct ChildView {
    child_id: String,
}

#[derive(Clone, ReadModel)]
#[readmodel(table = "parent_views", primary_key = ["parent_id"])]
struct ParentView {
    parent_id: String,
    #[readmodel(has_many = "ChildView", foreign_key = "parent_id")]
    children: Vec<ChildView>,
}

#[derive(Clone, ReadModel)]
#[readmodel(table = "todo_views", primary_key = ["tenant_id", "todo_id"])]
struct TodoView {
    tenant_id: String,
    todo_id: String,
    title: String,
    completed: bool,
    count: i64,
    optional_title: Option<String>,
    generated_id: String,
    #[readmodel(text)]
    status: TodoStatus,
}

#[test]
fn command_effects_compile_complete_typed_keys_and_assignments() {
    let _defaults = command_input_defaults! {
        input: UpdateTodo;
        default input.generated_id = uuid_v7();
    };
    let _effects = command_effects! {
        input: UpdateTodo;
        upsert TodoView {
            key { tenant_id: input.tenant_id, todo_id: input.todo_id },
            set {
                title: input.title,
                completed: false,
                optional_title: input.optional_title,
                generated_id: input.generated_id,
                status: constant(TodoStatus::Open)
            }
        };
        patch TodoView {
            key { tenant_id: input.tenant_id, todo_id: input.todo_id },
            set { title: input.title, count: constant(-1) }
        };
        delete TodoView {
            key { tenant_id: input.tenant_id, todo_id: input.todo_id }
        };
        invalidate TodoView;
    };
}

#[test]
fn nested_nullable_constants_and_ulid_default_compile() {
    let _defaults = command_input_defaults! {
        input: NestedUpdateTodo;
        default input.generated_id = ulid();
    };
    let _effects = command_effects! {
        input: NestedUpdateTodo;
        patch TodoView {
            key { tenant_id: input.tenant_id, todo_id: input.todo_id },
            set {
                title: input.details<TodoDetails>.title,
                optional_title: input.optional_details<TodoDetails>.title,
                generated_id: input.generated_id,
                status: constant(TodoStatus::Closed)
            }
        };
        patch TodoView {
            key { tenant_id: input.tenant_id, todo_id: input.todo_id },
            set { optional_title: null() }
        };
    };
}

#[test]
fn text_backed_unit_enum_round_trips_and_structured_values_fail_closed() {
    let view = TextKeyView {
        key: TextKey::Primary,
    };
    let row = view.to_row().unwrap();
    assert_eq!(row.get_serde::<TextKey>("key").unwrap(), TextKey::Primary);
    assert_eq!(
        view.primary_key().unwrap().values["key"],
        distributed::RowValue::String("Primary".into())
    );

    let structured = StructuredTextKeyView {
        key: StructuredTextKey::Payload {
            value: "not scalar text".into(),
        },
    };
    for error in [
        structured.to_row().unwrap_err(),
        structured.primary_key().unwrap_err(),
    ] {
        assert!(error
            .to_string()
            .contains("must serialize as a JSON string; got object"));
    }
}

#[test]
fn finite_confirmation_plan_reuses_the_projector_declaration() {
    let projector = SurfaceProjector::new("project_todos")
        .facts(["todo.changed"])
        .models(["TodoView"]);
    let _confirmations = command_confirmations! {
        input: UpdateTodo;
        confirm projector -> TodoView {
            key { tenant_id: input.tenant_id, todo_id: input.todo_id },
            partition: input.tenant_id
        };
    };
}

#[test]
fn command_effects_compile_typed_relationships() {
    let _effects = command_effects! {
        input: LinkTodo;
        link ParentView.children -> ChildView {
            source { parent_id: input.parent_id },
            target { child_id: input.child_id }
        };
        unlink ParentView.children -> ChildView {
            source { parent_id: input.parent_id },
            target { child_id: input.child_id }
        };
        invalidate ParentView.children {
            source { parent_id: input.parent_id }
        };
    };
}
