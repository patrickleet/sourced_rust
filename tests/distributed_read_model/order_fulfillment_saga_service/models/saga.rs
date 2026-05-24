use sourced_rust::{sourced, Entity, Snapshot};

/// Order fulfillment saga, keyed by order id. Tracks enough state to drive the
/// next step and to compensate. Internal digests are PascalCase (replay names);
/// the bus-facing steps are the lowercase `fulfillment.*` events.
#[derive(Default, Snapshot)]
pub struct OrderFulfillmentSaga {
    pub entity: Entity,
    pub order_id: String,
    pub sku: String,
    pub quantity: i64,
    pub amount_cents: i64,
    pub status: String,
    pub inventory_reserved: bool,
}

#[sourced(entity)]
impl OrderFulfillmentSaga {
    #[event("SagaStarted")]
    pub fn start(&mut self, order_id: String, sku: String, quantity: i64, amount_cents: i64) {
        self.entity.set_id(&order_id);
        self.order_id = order_id;
        self.sku = sku;
        self.quantity = quantity;
        self.amount_cents = amount_cents;
        self.status = "started".to_string();
    }

    #[event("SagaInventoryReserved")]
    pub fn inventory_reserved(&mut self) {
        self.status = "inventory_reserved".to_string();
        self.inventory_reserved = true;
    }

    #[event("SagaCompleted")]
    pub fn complete(&mut self) {
        self.status = "completed".to_string();
    }

    #[event("SagaCompensating")]
    pub fn compensate(&mut self, reason: String) {
        self.status = format!("compensating: {reason}");
    }

    #[event("SagaInventoryReleased")]
    pub fn inventory_released(&mut self) {
        self.inventory_reserved = false;
    }

    #[event("SagaCancelled")]
    pub fn cancel(&mut self) {
        self.status = "cancelled".to_string();
    }
}
