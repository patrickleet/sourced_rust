use distributed::emitter::EntityEmitter;
use distributed::{sourced, Entity};

pub struct Order {
    pub entity: Entity,
    pub emitter: EntityEmitter,
    pub order_id: String,
    pub customer: String,
    pub status: String,
}

impl Default for Order {
    fn default() -> Self {
        Self {
            entity: Entity::new(),
            emitter: EntityEmitter::default(),
            order_id: String::new(),
            customer: String::new(),
            status: String::new(),
        }
    }
}

#[sourced(entity, enqueue)]
impl Order {
    #[event("initialized")]
    pub fn create(&mut self, order_id: String, customer: String) {
        self.entity.set_id(&order_id);
        self.order_id = order_id;
        self.customer = customer;
        self.status = "created".into();
    }

    #[event("confirmed", when = self.status == "created")]
    pub fn confirm(&mut self) {
        self.status = "confirmed".into();
    }

    #[event("shipped", when = self.status == "confirmed")]
    pub fn ship(&mut self) {
        self.status = "shipped".into();
    }
}

/// Test custom emitter field with enqueue(my_emitter)
pub struct Notifier {
    pub entity: Entity,
    pub my_emitter: EntityEmitter,
    pub message: String,
}

impl Default for Notifier {
    fn default() -> Self {
        Self {
            entity: Entity::new(),
            my_emitter: EntityEmitter::default(),
            message: String::new(),
        }
    }
}

#[sourced(entity, enqueue(my_emitter))]
impl Notifier {
    #[event("sent")]
    pub fn send(&mut self, id: String, message: String) {
        self.entity.set_id(&id);
        self.message = message;
    }
}
