use distributed::emitter::EntityEmitter;
use distributed::{digest, enqueue, Entity};

/// An aggregate using both #[digest] and #[enqueue] — the typical pattern.
/// Events are recorded to the entity stream for replay AND queued for
/// in-process choreography emission.
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

impl Order {
    #[digest("initialized")]
    #[enqueue("order.initialized")]
    pub fn create(&mut self, order_id: String, customer: String) {
        self.entity.set_id(&order_id);
        self.order_id = order_id;
        self.customer = customer;
        self.status = "created".into();
    }

    #[digest("confirmed", when = self.status == "created")]
    #[enqueue("order.confirmed", when = self.status == "created")]
    pub fn confirm(&mut self) {
        self.status = "confirmed".into();
    }

    #[digest("shipped", when = self.status == "confirmed")]
    #[enqueue("order.shipped", when = self.status == "confirmed")]
    pub fn ship(&mut self) {
        self.status = "shipped".into();
    }
}

distributed::aggregate!(Order, entity {
    "initialized"(order_id, customer) => create,
    "confirmed"() => confirm(),
    "shipped"() => ship(),
});

/// An aggregate using #[enqueue] with a custom emitter field name.
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

impl Notifier {
    #[digest("sent")]
    #[enqueue(my_emitter, "notification.sent")]
    pub fn send(&mut self, id: String, message: String) {
        self.entity.set_id(&id);
        self.message = message;
    }
}

distributed::aggregate!(Notifier, entity {
    "sent"(id, message) => send,
});

/// An aggregate using only #[enqueue] without #[digest] — valid but uncommon.
/// Useful when you want local event choreography without persisted event history.
pub struct Ephemeral {
    pub entity: Entity,
    pub emitter: EntityEmitter,
    pub value: String,
}

impl Default for Ephemeral {
    fn default() -> Self {
        Self {
            entity: Entity::new(),
            emitter: EntityEmitter::default(),
            value: String::new(),
        }
    }
}

impl Ephemeral {
    #[enqueue("value.set")]
    pub fn set_value(&mut self, value: String) {
        self.value = value;
    }

    #[enqueue("value.cleared", when = !self.value.is_empty())]
    pub fn clear(&mut self) {
        self.value = String::new();
    }

    #[enqueue("value.checked")]
    pub fn check_with_tail_expression(&mut self) {
        self.tail_check()?
    }

    fn tail_check(&self) -> distributed::SourcedResult {
        Ok(())
    }
}
