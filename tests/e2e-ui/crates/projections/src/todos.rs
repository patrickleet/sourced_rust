//! Todo: mutations + portable handlers (spec language).

use distributed::mutation;
use distributed::portable_handlers;
use distributed::projection::lower::{EventualOnly, ProjectionDescriptor};
use distributed::Mutation;
use e2e_readmodels::Todos;
use todo_domain::{
    TodoArchivedDomainEvent, TodoCompletedDomainEvent, TodoCreatedDomainEvent,
    TodoForceArchivedDomainEvent, TodoPurgedDomainEvent, TodoReassignedDomainEvent,
    TodoRenamedDomainEvent, TodoReopenedDomainEvent, TodoState,
};

/// Mutation: complete-row upsert for Todo state transfer.
pub fn save_todo() -> Mutation<()> {
    mutation! {
        name: "save_todo";
        version: 1;
        upsert Todos from input.todo;
    }
}

/// Mutation: delete Todo by primary key (purge).
pub fn delete_todo() -> Mutation<()> {
    mutation! {
        name: "delete_todo";
        version: 1;
        delete Todos by_pk {
            todo_id: input.todo_id,
        };
    }
}

/// Canonical SAVE_TODO program.
pub fn save_todo_program() -> distributed::MutationProgram {
    save_todo().program().clone()
}

/// Canonical DELETE_TODO program.
pub fn delete_todo_program() -> distributed::MutationProgram {
    delete_todo().program().clone()
}

/// Lifecycle state events → [`save_todo`]; purge → [`delete_todo`].
portable_handlers! {
    pub const TODO_READS: ProjectionDescriptor<EventualOnly> = {
        name: "project_todos",
        version: 1,
        epoch: "e2e-ui-todos-v2",
        model: Todos,
        apply save_todo {
            on_event
                TodoCreatedDomainEvent,
                TodoRenamedDomainEvent,
                TodoCompletedDomainEvent,
                TodoReopenedDomainEvent,
                TodoReassignedDomainEvent,
                TodoArchivedDomainEvent,
                TodoForceArchivedDomainEvent
            as "todo"
        },
        apply delete_todo {
            on_deleted TodoPurgedDomainEvent as "todo_id"
        }
    };
}

/// Compiled handlers program (tests / service assertions).
pub fn todo_mutation_projection_program() -> Result<
    distributed::ProjectionProgram,
    distributed::ProjectionProgramError,
> {
    TODO_READS.program()
}

/// Partial client preview for `todo.complete`.
pub fn complete_preview() -> distributed::graphql::CommandProjectionPreview {
    distributed::state_preview! {
        TodoCompletedDomainEvent => TodoState {
            todo_id: input.todo_id,
            status: "completed",
            ..unknown
        }
    }
}

#[cfg(test)]
mod tests {
    use distributed::projection::placement::ProjectionExecutionClass;
    use distributed::{
        DomainEventBodyKind, MutationKind, RelationalReadModel, RowValue, TableMutation,
    };

    use super::*;
    use todo_domain::{Todo, TodoPurgedDomainEvent};

    #[test]
    fn todo_reads_program_is_built_from_mutation_ir() {
        let program = TODO_READS.program().unwrap();
        assert_eq!(program.arms().len(), 8);
        let upserts = program
            .arms()
            .iter()
            .filter(|arm| {
                arm.operations()
                    .iter()
                    .any(|op| op.kind() == distributed::ProjectionMutationKind::Upsert)
            })
            .count();
        let deletes = program
            .arms()
            .iter()
            .filter(|arm| {
                arm.operations()
                    .iter()
                    .any(|op| op.kind() == distributed::ProjectionMutationKind::Delete)
            })
            .count();
        assert_eq!(upserts, 7);
        assert_eq!(deletes, 1);
        assert_eq!(
            save_todo_program().operations()[0].kind(),
            MutationKind::Upsert
        );
        assert_eq!(
            delete_todo_program().operations()[0].kind(),
            MutationKind::Delete
        );
    }

    #[test]
    fn state_lifecycle_and_delete_define_one_causal_obligation_target() {
        let program = TODO_READS.program().unwrap();
        let inventory = TODO_READS.output_inventory().unwrap();
        assert_eq!(program.arms().len(), 8);
        assert_eq!(inventory.models.len(), 1);
        assert_eq!(inventory.models[0].model, "Todos");
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
    fn save_and_delete_mutations_are_event_independent() {
        let save = save_todo_program();
        let json = serde_json::to_value(&save).unwrap().to_string();
        assert!(!json.contains("event_name"));
        assert_eq!(save.operations()[0].kind(), MutationKind::Upsert);
        assert_eq!(
            delete_todo_program().operations()[0].kind(),
            MutationKind::Delete
        );
    }
}
