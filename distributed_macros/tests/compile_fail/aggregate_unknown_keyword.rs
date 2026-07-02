use distributed::{aggregate, Entity};

struct Todo {
    entity: Entity,
}

impl Todo {
    fn initialize(&mut self, id: String) -> distributed::SourcedResult<()> {
        let _ = id;
        Ok(())
    }
}

// The only keyword argument accepted after the entity field is
// `aggregate_type = "..."`; `aggregate_kind` is rejected with a pointed error.
aggregate!(Todo, entity, aggregate_kind = "todos" {
    "initialized"(id) => initialize,
});

fn main() {}
