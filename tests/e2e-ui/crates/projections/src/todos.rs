//! Todo projections: mutations + portable event bindings.
//!
//! Author surface is intentional:
//! 1. `mutation!` programs (event-free)
//! 2. arms binding domain events → those mutations
//! 3. `mutation_projector!` mount (framework owns resolve/lower/inventory)

use distributed::domain_event::DomainEventContract;
use distributed::mutation;
use distributed::projection::lower::{EventualOnly, ProjectionDescriptor};
use distributed::{
    arm_delete_pk_from_envelope, arms_state_upsert_for_model, build_mutation_projector_program,
    mutation_projector, Mutation, MutationProgram, ProjectionPartition, ProjectionProgram,
    ProjectionProgramError,
};
use e2e_readmodels::Todos;
use todo_domain::{
    TodoArchivedDomainEvent, TodoCompletedDomainEvent, TodoCreatedDomainEvent,
    TodoForceArchivedDomainEvent, TodoPurgedDomainEvent, TodoReassignedDomainEvent,
    TodoRenamedDomainEvent, TodoReopenedDomainEvent, TodoState,
};

/// Complete-row upsert for Todo state transfer.
pub fn save_todo() -> Mutation<()> {
    mutation! {
        name: "save_todo";
        version: 1;
        upsert Todos from input.todo;
    }
}

/// Delete by primary key for Todo purge.
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
pub fn save_todo_program() -> MutationProgram {
    save_todo().program().clone()
}

/// Canonical DELETE_TODO program.
pub fn delete_todo_program() -> MutationProgram {
    delete_todo().program().clone()
}

/// Projector program: lifecycle state events → SAVE_TODO, purge → DELETE_TODO.
pub fn todo_mutation_projection_program() -> Result<ProjectionProgram, ProjectionProgramError> {
    let save = save_todo_program();
    let mut arms = arms_state_upsert_for_model::<Todos>(
        &save,
        "todo",
        &[
            ("todo-created", &TodoCreatedDomainEvent::descriptor()),
            ("todo-renamed", &TodoRenamedDomainEvent::descriptor()),
            ("todo-completed", &TodoCompletedDomainEvent::descriptor()),
            ("todo-reopened", &TodoReopenedDomainEvent::descriptor()),
            ("todo-reassigned", &TodoReassignedDomainEvent::descriptor()),
            ("todo-archived", &TodoArchivedDomainEvent::descriptor()),
            (
                "todo-force-archived",
                &TodoForceArchivedDomainEvent::descriptor(),
            ),
        ],
    )
    .map_err(|e| ProjectionProgramError::InvalidOperation {
        operation: "project_todos".into(),
        reason: e.to_string(),
    })?;
    arms.push(
        arm_delete_pk_from_envelope(
            "todo-purged",
            &TodoPurgedDomainEvent::descriptor(),
            delete_todo_program(),
            "todo_id",
        )
        .map_err(|e| ProjectionProgramError::InvalidOperation {
            operation: "project_todos".into(),
            reason: e.to_string(),
        })?,
    );
    build_mutation_projector_program("project_todos", 1, ProjectionPartition::Unit, arms)
}

mutation_projector! {
    pub const TODO_READS: ProjectionDescriptor<EventualOnly> = {
        name: "project_todos",
        version: 1,
        epoch: "e2e-ui-todos-v2",
        model: Todos,
        program: todo_mutation_projection_program,
    };
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
    use distributed::domain_event::DomainEventContract;
    use distributed::projection::placement::ProjectionExecutionClass;
    use distributed::{
        DomainEventBodyKind, MutationKind, RelationalReadModel, RowValue, TableMutation,
    };

    use super::*;
    use todo_domain::{Todo, TodoPurgedDomainEvent};

    #[test]
    fn todo_reads_program_is_built_from_mutation_ir() {
        let program = TODO_READS.program().unwrap();
        let mutation_program = todo_mutation_projection_program().unwrap();
        assert_eq!(
            program.canonical_bytes().unwrap(),
            mutation_program.canonical_bytes().unwrap()
        );
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
        assert_eq!(upserts, 7, "seven state arms rewrite SAVE_TODO");
        assert_eq!(deletes, 1, "one purge arm rewrites DELETE_TODO");
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
        use distributed::MUTATION_PROGRAM_IR_VERSION;

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
}
