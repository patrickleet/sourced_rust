use distributed_macros::enqueue;

struct Entity;

struct EntityEmitter;

impl EntityEmitter {
    fn enqueue_with<T>(&mut self, _: &str, _: &T) -> distributed::SourcedResult<()> {
        Ok(())
    }
}

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
