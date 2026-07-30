//! Todo domain-event → read-model projections via **mutation IR**.
//!
//! `SAVE_TODO` / `DELETE_TODO` are the public authoring model. The dual-path
//! `TODO_READS` descriptor's program/resolve factories are built from those
//! mutations (not from event-owning `projection!`).

use distributed::domain_event::DomainEventContract;
use distributed::mutation;
use distributed::projection::lower::{
    EventualOnly, ProjectionDescriptor, ProjectionLoweringError, ProjectionOutputInventory,
};
use distributed::{
    body_field_binding, descriptor_from_factories, envelope_binding, inventory_single_model,
    lower_single_model, program_from_mutation_arms, resolve_mutation_program, Mutation,
    MutationEventBinding, MutationProgram, MutationProjectionArm, ProjectionEnvelopeField,
    ProjectionPartition, ProjectionProgram, ProjectionProgramError, ProjectionValueType,
    ResolvedProjectionPlan,
};
use distributed::DomainEventOccurrence;

use e2e_readmodels::Todos;
use todo_domain::{
    TodoArchivedDomainEvent, TodoCompletedDomainEvent, TodoCreatedDomainEvent,
    TodoDomainIdentity, TodoForceArchivedDomainEvent, TodoPurgedDomainEvent,
    TodoReassignedDomainEvent, TodoRenamedDomainEvent, TodoReopenedDomainEvent, TodoState,
};

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

fn todo_state_bindings() -> Result<Vec<distributed::MutationInputBinding>, distributed::MutationProgramError>
{
    Ok(vec![
        body_field_binding(
            ["todo", "todo_id"],
            ["todo_id"],
            ProjectionValueType::String,
        )?,
        body_field_binding(
            ["todo", "owner_id"],
            ["owner_id"],
            ProjectionValueType::String,
        )?,
        body_field_binding(["todo", "title"], ["title"], ProjectionValueType::String)?,
        body_field_binding(["todo", "status"], ["status"], ProjectionValueType::String)?,
        body_field_binding(
            ["todo", "assignee_id"],
            ["assignee_id"],
            ProjectionValueType::String,
        )?,
    ])
}

fn state_arm(
    arm_id: &'static str,
    descriptor: &distributed::DomainEventDescriptor,
) -> Result<MutationProjectionArm, distributed::MutationProgramError> {
    let selector = distributed::ProjectionEventSelector::try_from_descriptor(descriptor)
        .map_err(distributed::MutationProgramError::from)?;
    let binding =
        MutationEventBinding::try_new(selector, todo_state_bindings()?, save_todo_program())?;
    Ok(MutationProjectionArm { arm_id, binding })
}

fn purge_arm() -> Result<MutationProjectionArm, distributed::MutationProgramError> {
    let selector = distributed::ProjectionEventSelector::try_from_descriptor(
        &TodoPurgedDomainEvent::descriptor(),
    )
    .map_err(distributed::MutationProgramError::from)?;
    let inputs = vec![envelope_binding(
        ["todo_id"],
        ProjectionEnvelopeField::AggregateId,
    )?];
    let binding = MutationEventBinding::try_new(selector, inputs, delete_todo_program())?;
    Ok(MutationProjectionArm {
        arm_id: "todo-purged",
        binding,
    })
}

/// Build the dual-path projection program from SAVE_TODO / DELETE_TODO.
pub fn todo_mutation_projection_program() -> Result<ProjectionProgram, ProjectionProgramError> {
    let arms = vec![
        state_arm("todo-created", &TodoCreatedDomainEvent::descriptor())
            .map_err(|e| ProjectionProgramError::InvalidOperation {
                operation: "todo mutation arm".into(),
                reason: e.to_string(),
            })?,
        state_arm("todo-renamed", &TodoRenamedDomainEvent::descriptor()).map_err(|e| {
            ProjectionProgramError::InvalidOperation {
                operation: "todo mutation arm".into(),
                reason: e.to_string(),
            }
        })?,
        state_arm("todo-completed", &TodoCompletedDomainEvent::descriptor()).map_err(|e| {
            ProjectionProgramError::InvalidOperation {
                operation: "todo mutation arm".into(),
                reason: e.to_string(),
            }
        })?,
        state_arm("todo-reopened", &TodoReopenedDomainEvent::descriptor()).map_err(|e| {
            ProjectionProgramError::InvalidOperation {
                operation: "todo mutation arm".into(),
                reason: e.to_string(),
            }
        })?,
        state_arm("todo-reassigned", &TodoReassignedDomainEvent::descriptor()).map_err(|e| {
            ProjectionProgramError::InvalidOperation {
                operation: "todo mutation arm".into(),
                reason: e.to_string(),
            }
        })?,
        state_arm("todo-archived", &TodoArchivedDomainEvent::descriptor()).map_err(|e| {
            ProjectionProgramError::InvalidOperation {
                operation: "todo mutation arm".into(),
                reason: e.to_string(),
            }
        })?,
        state_arm(
            "todo-force-archived",
            &TodoForceArchivedDomainEvent::descriptor(),
        )
        .map_err(|e| ProjectionProgramError::InvalidOperation {
            operation: "todo mutation arm".into(),
            reason: e.to_string(),
        })?,
        purge_arm().map_err(|e| ProjectionProgramError::InvalidOperation {
            operation: "todo mutation arm".into(),
            reason: e.to_string(),
        })?,
    ];
    program_from_mutation_arms("project_todos", 1, ProjectionPartition::Unit, &arms).map_err(
        |e| ProjectionProgramError::InvalidOperation {
            operation: "project_todos".into(),
            reason: e.to_string(),
        },
    )
}

fn todo_program_factory() -> Result<ProjectionProgram, ProjectionProgramError> {
    todo_mutation_projection_program()
}

fn todo_resolve(
    occurrence: &DomainEventOccurrence,
) -> Result<ResolvedProjectionPlan, ProjectionProgramError> {
    let program = todo_mutation_projection_program()?;
    resolve_mutation_program(&program, occurrence)
}

fn todo_lower(
    plan: &ResolvedProjectionPlan,
) -> Result<distributed::projection::lower::LoweredProjectionPlan, ProjectionLoweringError> {
    lower_single_model::<Todos>(plan)
}

fn todo_inventory() -> Result<ProjectionOutputInventory, ProjectionLoweringError> {
    inventory_single_model::<Todos>()
}

/// Mutation-backed Todo projector mount (program/resolve from SAVE_TODO/DELETE_TODO).
pub const TODO_READS: ProjectionDescriptor<EventualOnly> = descriptor_from_factories(
    "project_todos",
    1,
    "e2e-ui-todos-v2",
    todo_program_factory,
    todo_resolve,
    todo_lower,
    todo_inventory,
);

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
    use distributed::{DomainEventBodyKind, MutationKind, RelationalReadModel, RowValue, TableMutation};

    use super::*;
    use todo_domain::{Todo, TodoPurgedDomainEvent, TodoStatus};

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
        assert_eq!(save_todo_program().operations()[0].kind(), MutationKind::Upsert);
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
