use distributed::{sourced, Entity};

struct Todo {
    entity: Entity,
}

// `#[sourced]` requires the entity field name as its first argument.
#[sourced()]
impl Todo {
    #[event("initialized")]
    pub fn initialize(&mut self, id: String) {
        let _ = id;
    }
}

fn main() {}
