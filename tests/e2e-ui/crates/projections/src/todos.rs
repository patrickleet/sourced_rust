//! Todo domain-event projections into query models.

use distributed::projection;
use distributed::projection::lower::{EventualOnly, ProjectionDescriptor};
#[cfg(test)]
use distributed::Entity;

use e2e_readmodels::Todos;
use todo_domain::{TodoCompletedDomainEvent, TodoDomainIdentity, TodoState};

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
