use sourced_rust::emitter::EntityEmitter;
use sourced_rust::{enqueue, sourced, Entity};

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

#[sourced(entity)]
impl Order {
    #[event("OrderCreated")]
    #[enqueue("OrderCreated")]
    pub fn create(&mut self, order_id: String, customer: String) {
        self.entity.set_id(&order_id);
        self.order_id = order_id;
        self.customer = customer;
        self.status = "created".into();
    }

    #[event("OrderConfirmed", when = self.status == "created")]
    #[enqueue("OrderConfirmed", when = self.status == "created")]
    pub fn confirm(&mut self) {
        self.status = "confirmed".into();
    }

    #[event("OrderShipped", when = self.status == "confirmed")]
    #[enqueue("OrderShipped", when = self.status == "confirmed")]
    pub fn ship(&mut self) {
        self.status = "shipped".into();
    }
}
