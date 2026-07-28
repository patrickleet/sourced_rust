use distributed::{sourced, Entity};

#[derive(Default)]
struct Todo {
    entity: Entity,
}

#[sourced(entity, aggregate_type = "todo")]
impl Todo {
    #[event("todo.completed", domain = event)]
    fn complete(&mut self) {
        return;
    }
}

fn main() {}
