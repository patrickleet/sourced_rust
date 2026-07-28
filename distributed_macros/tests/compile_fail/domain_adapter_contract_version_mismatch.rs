use distributed::{
    domain_event::DomainEventContract, sourced, DomainEvent, DomainEventBodyDescriptor,
    DomainEventBodyKind, DomainEventDescriptor, Entity,
};
use serde::Serialize;

#[derive(Serialize)]
struct TodoCompleted;

impl DomainEvent for TodoCompleted {
    const DESCRIPTOR: DomainEventDescriptor = DomainEventDescriptor {
        name: std::borrow::Cow::Borrowed("todo.completed"),
        version: 1,
        body: DomainEventBodyDescriptor::distributed_json(
            DomainEventBodyKind::Event,
            "TodoCompleted",
            1,
            "todo-completed-v1",
            "sha256:1111111111111111111111111111111111111111111111111111111111111111",
        ),
    };
}

impl DomainEventContract for TodoCompleted {
    type Body = Self;
    const EVENT_NAME: &'static str = "todo.completed";
    const EVENT_VERSION: u64 = 2;

    fn descriptor() -> DomainEventDescriptor {
        Self::DESCRIPTOR.clone()
    }
}

#[derive(Default)]
struct Todo {
    entity: Entity,
}

fn capture(_todo: &Todo, _event: &TodoReplayEvent) -> TodoCompleted {
    TodoCompleted
}

#[sourced(entity, events = "TodoReplayEvent", aggregate_type = "todo")]
impl Todo {
    #[event("todo.completed", version = 1, domain = with(TodoCompleted, capture))]
    fn complete(&mut self) {}
}

fn main() {}
