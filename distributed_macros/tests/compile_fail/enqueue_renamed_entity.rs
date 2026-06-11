use distributed::emitter::EntityEmitter;
use distributed::{enqueue, Entity};

// The entity field is named `state`, not `entity`. Without telling `#[enqueue]`
// about the rename (via `entity = state`), the generated replay guard references
// the non-existent `self.entity` field. The fix is `#[enqueue("...", entity = state)]`.
struct Order {
    state: Entity,
    emitter: EntityEmitter,
}

impl Order {
    #[enqueue("order.initialized")]
    pub fn create(&mut self, id: String) {
        let _ = id;
    }
}

fn main() {}
