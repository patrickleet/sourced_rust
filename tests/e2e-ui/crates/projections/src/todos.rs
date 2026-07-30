//! Todo domain-event projections into query models.
//!
//! Mutation IR (`SAVE_TODO` / `DELETE_TODO`) is the public projector authoring
//! model. The legacy `projection!` descriptor remains the dual-path runtime
//! mount until cutover removes event-owning projection macros.

use distributed::mutation;
use distributed::projection;
use distributed::projection::lower::{EventualOnly, ProjectionDescriptor};
#[cfg(test)]
use distributed::Entity;
use distributed::{Mutation, MutationProgram};

use e2e_readmodels::Todos;
use todo_domain::{TodoCompletedDomainEvent, TodoDomainIdentity, TodoState};

/// Event-independent complete-row upsert for Todo state transfer.
pub fn save_todo() -> Mutation<()> {
    mutation! {
        name: "save_todo";
        version: 1;
        upsert Todos from input.todo;
    }
}

/// Event-independent delete by primary key for Todo purge.
pub fn delete_todo() -> Mutation<()> {
    mutation! {
        name: "delete_todo";
        version: 1;
        delete Todos by_pk {
            todo_id: input.todo_id,
        };
    }
}

/// Canonical SAVE_TODO mutation program (event-independent).
pub fn save_todo_program() -> MutationProgram {
    save_todo().program().clone()
}

/// Canonical DELETE_TODO mutation program (event-independent).
pub fn delete_todo_program() -> MutationProgram {
    delete_todo().program().clone()
}

/// One modeled state-transfer lifecycle plus explicit physical deletion.
pub const TODO_READS: ProjectionDescriptor<EventualOnly> = projection! {
    name: "project_todos";
    version: 1;
    epoch: "e2e-ui-todos-v2";
    partition: unit;

    on [
        "todo.created",
        "todo.renamed",
        "todo.completed",
        "todo.reopened",
        "todo.reassigned",
        "todo.archived",
        "todo.force_archived"
    ] version 1 (state: TodoState) {
        upsert Todos from state as todo;
    }

    on "todo.purged" version 1 (deleted: TodoDomainIdentity) {
        delete Todos {
            key { todo_id: envelope.aggregate_id }
        };
    }
};

/// Partial client preview for `todo.complete`.
///
/// The full authoritative `TodoState` is captured only after the aggregate
/// transition. Before dispatch, only identity and the constant status are
/// proven; task 16/20 lower this to conditional patch plus missing-row recovery.
pub fn complete_preview() -> distributed::graphql::CommandProjectionPreview {
    distributed::state_preview! {
        TodoCompletedDomainEvent => TodoState {
            todo_id: input.todo_id,
            status: "completed",
            ..unknown
        }
    }
}

#[derive(Default)]
#[cfg(test)]
struct SparseTodo {
    entity: Entity,
    completed: bool,
}

#[cfg(test)]
#[distributed::sourced(entity, events = "SparseTodoEvent", aggregate_type = "todo")]
impl SparseTodo {
    #[event("todo.fixture-created")]
    fn create(&mut self, todo_id: String) {
        self.entity.set_id(todo_id);
    }

    #[event("todo.completed", version = 1, domain = event)]
    fn complete(&mut self) {
        self.completed = true;
    }
}

/// Deliberately unmounted alternative proving sparse identity promotion.
#[cfg(test)]
const SPARSE_TODO_COMPLETION: ProjectionDescriptor<EventualOnly> = projection! {
    name: "fixture_sparse_todo_completion";
    version: 1;
    epoch: "fixture-only";
    partition: envelope.aggregate_id;

    on SparseTodoCompletedDomainEvent(event) {
        patch Todos {
            key { todo_id: envelope.aggregate_id },
            set { status: "completed" }
        };
    }
};

#[cfg(test)]
mod tests {
    use distributed::domain_event::DomainEventContract;
    use distributed::projection::placement::ProjectionExecutionClass;
    use distributed::{DomainEventBodyKind, RelationalReadModel, RowValue, TableMutation};

    use super::*;
    use todo_domain::{Todo, TodoPurgedDomainEvent, TodoStatus};

    #[test]
    fn state_lifecycle_and_delete_define_one_causal_obligation_target() {
        let program = TODO_READS.program().unwrap();
        let inventory = TODO_READS.output_inventory().unwrap();

        assert_eq!(program.arms().len(), 8);
        assert_eq!(inventory.models.len(), 1);
        assert_eq!(inventory.models[0].model, "Todos");
        assert_eq!(inventory.models[0].storage, "todos");
        assert_eq!(
            TODO_READS.eventual().execution_class(),
            ProjectionExecutionClass::Causal
        );
        assert_eq!(Todos::schema().table_name, "todos");
    }

    #[test]
    fn actual_complete_is_an_authoritative_full_row_upsert() {
        let mut todo = Todo::default();
        todo.create("todo-1", "alice", "Read").unwrap();
        todo.reassign("alice", "bob").unwrap();
        todo.complete("alice").unwrap();
        let occurrence = todo.entity.pending_domain_events().last().unwrap();
        let lowered = TODO_READS
            .server_executor()
            .unwrap()
            .plan(occurrence)
            .unwrap();

        let TableMutation::UpsertRow(row) = &lowered.write_plan.mutations[0] else {
            panic!("complete must lower to authoritative upsert");
        };
        let projected_fields = lowered.resolved.mutations()[0]
            .fields()
            .iter()
            .map(|field| field.name())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            projected_fields,
            ["assignee_id", "owner_id", "status", "title", "todo_id"]
                .into_iter()
                .collect()
        );
        assert_eq!(
            row.values.get("status"),
            Some(&RowValue::String("completed".into()))
        );
        assert_eq!(
            row.values.get("assignee_id"),
            Some(&RowValue::String("bob".into()))
        );
    }

    #[test]
    fn complete_preview_declares_only_proven_identity_and_status() {
        let value = serde_json::to_value(complete_preview()).unwrap();
        let fields = value["fields"].as_array().unwrap();
        assert_eq!(fields.len(), 2);
        assert!(fields.iter().any(|field| {
            field["body_path"] == serde_json::json!(["status"])
                && field["source"]["kind"] == "constant"
        }));
        assert!(fields.iter().any(|field| {
            field["body_path"] == serde_json::json!(["todo_id"])
                && field["source"]["kind"] == "input_path"
        }));
    }

    #[test]
    fn purge_descriptor_and_plan_use_explicit_tombstone_identity() {
        let mut todo = Todo::default();
        todo.create("todo-1", "alice", "Read").unwrap();
        todo.purge("alice").unwrap();
        let occurrence = todo.entity.pending_domain_events().last().unwrap();
        assert_eq!(
            occurrence.descriptor().body.kind,
            DomainEventBodyKind::Deletion
        );
        assert_eq!(
            occurrence.descriptor(),
            &TodoPurgedDomainEvent::descriptor()
        );
        let lowered = TODO_READS
            .server_executor()
            .unwrap()
            .plan(occurrence)
            .unwrap();
        let TableMutation::DeleteRow(delete) = &lowered.write_plan.mutations[0] else {
            panic!("purge must lower to delete");
        };
        assert_eq!(
            delete.key.get("todo_id"),
            Some(&RowValue::String("todo-1".into()))
        );
    }

    #[test]
    fn save_and_delete_mutations_are_event_independent_and_versioned() {
        use distributed::{MutationKind, MUTATION_PROGRAM_IR_VERSION};

        let save = save_todo_program();
        let delete = delete_todo_program();
        assert_eq!(save.ir_version(), MUTATION_PROGRAM_IR_VERSION);
        assert_eq!(save.operations().len(), 1);
        assert_eq!(save.operations()[0].kind(), MutationKind::Upsert);
        assert_eq!(save.operations()[0].target().model(), "Todos");
        assert_eq!(delete.operations()[0].kind(), MutationKind::Delete);
        let save_json = serde_json::to_value(&save).unwrap().to_string();
        assert!(!save_json.contains("event_name"));
        assert!(!save_json.contains("\"selector\""));
        // Sugar and explicit primary-key conflict canonicalize identically.
        let explicit = mutation! {
            name: "save_todo";
            version: 1;
            upsert Todos one {
                object: input.todo,
                conflict: primary_key,
                update: all_input_fields,
            };
        };
        assert_eq!(
            save.id().unwrap().to_string(),
            explicit.id().unwrap().to_string()
        );
    }

    #[test]
    fn mutation_rewrite_matches_projection_arm_kinds_for_state_and_purge() {
        use distributed::{
            body_field_binding, MutationEventBinding, ProjectionEventSelector, ProjectionValueType,
            DomainEventBodyKind, DOMAIN_EVENT_OCCURRENCE_VERSION,
        };

        let selector = ProjectionEventSelector::try_new(
            DOMAIN_EVENT_OCCURRENCE_VERSION,
            "todo.completed",
            1,
            DomainEventBodyKind::State,
            "TodoState",
            1,
            "urn:distributed:test:todo-state:v1",
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "distributed-json",
            1,
        )
        .unwrap();
        let bindings = vec![
            body_field_binding(["todo", "todo_id"], ["todo_id"], ProjectionValueType::String)
                .unwrap(),
            body_field_binding(["todo", "owner_id"], ["owner_id"], ProjectionValueType::String)
                .unwrap(),
            body_field_binding(["todo", "title"], ["title"], ProjectionValueType::String).unwrap(),
            body_field_binding(["todo", "status"], ["status"], ProjectionValueType::String)
                .unwrap(),
            body_field_binding(
                ["todo", "assignee_id"],
                ["assignee_id"],
                ProjectionValueType::String,
            )
            .unwrap(),
        ];
        let binding =
            MutationEventBinding::try_new(selector, bindings, save_todo_program()).unwrap();
        let arm = binding.to_projection_arm("completed").unwrap();
        assert_eq!(arm.operations().len(), 1);
        assert_eq!(
            arm.operations()[0].kind(),
            distributed::ProjectionMutationKind::Upsert
        );
        assert_eq!(arm.operations()[0].target().model(), "Todos");
    }

    #[test]
    fn sparse_identity_promotion_is_isolated_and_never_dual_writes() {
        let mut todo = SparseTodo::default();
        todo.create("todo-1".into()).unwrap();
        assert!(todo.entity.pending_domain_events().is_empty());
        todo.complete().unwrap();
        assert_eq!(todo.entity.pending_domain_events().len(), 1);

        let occurrence = todo.entity.pending_domain_events().last().unwrap();
        let sparse = SPARSE_TODO_COMPLETION
            .server_executor()
            .unwrap()
            .plan(occurrence)
            .unwrap();
        assert!(matches!(
            sparse.write_plan.mutations.as_slice(),
            [TableMutation::PatchRow(_)]
        ));
        assert_ne!(
            SPARSE_TODO_COMPLETION.program_id().unwrap(),
            TODO_READS.program_id().unwrap()
        );

        let mut authoritative = Todo::default();
        authoritative.create("todo-2", "alice", "Read").unwrap();
        authoritative.complete("alice").unwrap();
        let actual = authoritative.entity.pending_domain_events().last().unwrap();
        assert!(TODO_READS.resolve(actual).is_ok());
        assert!(SPARSE_TODO_COMPLETION.resolve(actual).is_err());
        assert_eq!(authoritative.status, TodoStatus::Completed);
    }
}
