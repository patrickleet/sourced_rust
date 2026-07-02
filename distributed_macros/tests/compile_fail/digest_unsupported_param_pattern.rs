use distributed::{digest, Entity};

struct Todo {
    entity: Entity,
}

impl Todo {
    // `_` has no name to record in the event payload, so the parameter would
    // silently vanish from the digest call. Parameters must be plain
    // identifiers.
    #[digest("initialized")]
    pub fn initialize(&mut self, _: String) {}
}

fn main() {}
