use distributed::{sourced, Entity};

struct Todo {
    entity: Entity,
}

#[sourced(entity, aggregate_type = "todo")]
impl Todo {
    #[event("todo.completed", domain)]
    fn complete(&mut self) {}
}

fn main() {}
