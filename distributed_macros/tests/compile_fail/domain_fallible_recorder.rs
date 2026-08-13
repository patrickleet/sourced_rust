use distributed::{sourced, Entity};

struct Todo {
    entity: Entity,
}

#[sourced(entity, aggregate_type = "todo")]
impl Todo {
    #[event("todo.completed", domain = event)]
    fn complete(&mut self) -> distributed::SourcedResult {
        Ok(())
    }
}

fn main() {}
