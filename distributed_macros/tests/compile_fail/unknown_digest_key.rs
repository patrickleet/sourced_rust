use distributed::{digest, Entity};

struct Todo {
    entity: Entity,
}

impl Todo {
    // Typo: `versoin` is not a recognized key. The diagnostic should name the
    // bad key and list the valid ones.
    #[digest("x", versoin = 2)]
    pub fn initialize(&mut self, id: String) {
        let _ = id;
    }
}

fn main() {}
