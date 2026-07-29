//! Domain-event projections for Todo query models.

use distributed::domain_event::{DomainEventBodyContract, DomainEventContract};
use distributed::projection::lower::{
    EventualOnly, ProjectionDeletionMetadata, ProjectionDescriptor,
};
#[cfg(test)]
use distributed::Entity;
use distributed::{
    projection, DomainDeletion, DomainEventBodyDescriptor, DomainEventBodyKind,
    DomainEventDescriptor,
};
use serde::{Deserialize, Serialize};

use crate::{TodoState, Todos};

macro_rules! state_event_contract {
    ($name:ident, $event:literal) => {
        pub enum $name {}

        impl DomainEventContract for $name {
            const EVENT_NAME: &'static str = $event;
            const EVENT_VERSION: u64 = 1;

            fn descriptor() -> DomainEventDescriptor {
                DomainEventDescriptor::state::<TodoState>($event, 1)
            }
        }

        impl DomainEventBodyContract<TodoState> for $name {}
    };
}

state_event_contract!(TodoCreatedDomainEvent, "todo.created");
state_event_contract!(TodoRenamedDomainEvent, "todo.renamed");
state_event_contract!(TodoCompletedDomainEvent, "todo.completed");
state_event_contract!(TodoReopenedDomainEvent, "todo.reopened");
state_event_contract!(TodoReassignedDomainEvent, "todo.reassigned");
state_event_contract!(TodoArchivedDomainEvent, "todo.archived");
state_event_contract!(TodoForceArchivedDomainEvent, "todo.force_archived");

/// Stable public identity body for the sourced Todo deletion occurrence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoDeletionIdentity {
    pub aggregate_id: String,
}

impl ProjectionDeletionMetadata for TodoDeletionIdentity {
    const BODY_TYPE_NAME: &'static str = "DomainDeletion<TodoDomainIdentity>";
    const BODY_SCHEMA: &'static str = "distributed.schema/v1|role=15:domain_deletion|type=34:DomainDeletion<TodoDomainIdentity>|version=1|serde=0:|fields=2:3:key|18:TodoDomainIdentity|0:|11:incarnation|3:u64|0:";
    const BODY_FINGERPRINT: &'static str =
        "sha256:21a004c5d43e73d50b437a661028fe568d22aea9cea96c7ae01f99aefb271f2a";
}

pub enum TodoPurgedDomainEvent {}

impl DomainEventContract for TodoPurgedDomainEvent {
    const EVENT_NAME: &'static str = "todo.purged";
    const EVENT_VERSION: u64 = 1;

    fn descriptor() -> DomainEventDescriptor {
        DomainEventDescriptor {
            name: Self::EVENT_NAME.into(),
            version: Self::EVENT_VERSION,
            body: DomainEventBodyDescriptor::distributed_json(
                DomainEventBodyKind::Deletion,
                TodoDeletionIdentity::BODY_TYPE_NAME,
                1,
                TodoDeletionIdentity::BODY_SCHEMA,
                TodoDeletionIdentity::BODY_FINGERPRINT,
            ),
        }
    }
}

impl DomainEventBodyContract<DomainDeletion<TodoDeletionIdentity>> for TodoPurgedDomainEvent {}

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

    on "todo.purged" version 1 (deleted: TodoDeletionIdentity) {
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
    use distributed::projection::placement::ProjectionExecutionClass;
    use distributed::{DomainEventBodyKind, RelationalReadModel, RowValue, TableMutation};

    use super::*;
    use crate::{Todo, TodoStatus};

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
        assert_eq!(
            Todo::purged_domain_event_descriptor(),
            TodoPurgedDomainEvent::descriptor()
        );
        let mut todo = Todo::default();
        todo.create("todo-1", "alice", "Read").unwrap();
        todo.purge("alice").unwrap();
        let occurrence = todo.entity.pending_domain_events().last().unwrap();
        assert_eq!(
            occurrence.descriptor().body.kind,
            DomainEventBodyKind::Deletion
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
