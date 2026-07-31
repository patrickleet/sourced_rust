//! Todo: mutations + portable handlers.

use distributed::mutation;
use distributed::portable_handlers;
use distributed::projection::lower::{EventualOnly, ProjectionDescriptor};
use distributed::Mutation;
use e2e_readmodels::Todos;
use todo_domain::{
    TodoArchivedDomainEvent, TodoCompletedDomainEvent, TodoCreatedDomainEvent,
    TodoForceArchivedDomainEvent, TodoPurgedDomainEvent, TodoReassignedDomainEvent,
    TodoRenamedDomainEvent, TodoReopenedDomainEvent,
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

// Lifecycle state events → [`save_todo`]; purge → [`delete_todo`].
portable_handlers! {
    pub const TODOS: ProjectionDescriptor<EventualOnly> = {
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

#[cfg(test)]
mod tests {
    use distributed::domain_event::DomainEventContract;
    use distributed::projection::placement::ProjectionExecutionClass;
    use distributed::{DomainEventBodyKind, MutationKind, RelationalReadModel, RowValue, TableMutation};

    use super::*;
    use todo_domain::{Todo, TodoPurgedDomainEvent};

    #[test]
    fn todo_handlers_apply_save_and_delete_mutations() {
        let program = TODOS.program().unwrap();
        assert_eq!(program.arms().len(), 8);
        assert_eq!(save_todo().program().operations()[0].kind(), MutationKind::Upsert);
        assert_eq!(delete_todo().program().operations()[0].kind(), MutationKind::Delete);
    }

    #[test]
    fn complete_lowers_to_full_row_upsert() {
        let mut todo = Todo::default();
        todo.create("todo-1", "alice", "Read").unwrap();
        todo.reassign("alice", "bob").unwrap();
        todo.complete("alice").unwrap();
        let occurrence = todo.entity.pending_domain_events().last().unwrap();
        let lowered = TODOS.server_executor().unwrap().plan(occurrence).unwrap();
        let TableMutation::UpsertRow(row) = &lowered.write_plan.mutations[0] else {
            panic!("expected upsert");
        };
        assert_eq!(row.values.get("status"), Some(&RowValue::String("completed".into())));
        assert_eq!(Todos::schema().table_name, "todos");
        assert_eq!(
            TODOS.eventual().execution_class(),
            ProjectionExecutionClass::Causal
        );
    }

    #[test]
    fn purge_lowers_to_delete() {
        let mut todo = Todo::default();
        todo.create("todo-1", "alice", "Read").unwrap();
        todo.purge("alice").unwrap();
        let occurrence = todo.entity.pending_domain_events().last().unwrap();
        assert_eq!(occurrence.descriptor().body.kind, DomainEventBodyKind::Deletion);
        assert_eq!(occurrence.descriptor(), &TodoPurgedDomainEvent::descriptor());
        let lowered = TODOS.server_executor().unwrap().plan(occurrence).unwrap();
        let TableMutation::DeleteRow(delete) = &lowered.write_plan.mutations[0] else {
            panic!("expected delete");
        };
        assert_eq!(delete.key.get("todo_id"), Some(&RowValue::String("todo-1".into())));
    }

    #[test]
    fn mutations_are_event_free() {
        let json = serde_json::to_value(save_todo().program()).unwrap().to_string();
        assert!(!json.contains("event_name"));
    }
}
