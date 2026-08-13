use distributed::{sourced, Entity};
use serde::Serialize;

#[derive(Serialize, distributed_macros::DomainEvent)]
#[domain_event(name = "todo.completed", version = 1)]
struct TodoCompleted {
    todo_id: String,
}

#[derive(Default)]
struct Todo {
    entity: Entity,
}

fn wrong_adapter(_todo: &Todo, _event: &str) -> TodoCompleted {
    TodoCompleted {
        todo_id: String::new(),
    }
}

#[sourced(entity, events = "TodoReplayEvent", aggregate_type = "todo")]
impl Todo {
    #[event(
        "todo.completed",
        domain = with(TodoCompleted, wrong_adapter)
    )]
    fn complete(&mut self) {}
}

fn main() {}
