use sourced_rust::{sourced, Entity, Snapshot};

/// Payment per order. Declines amounts over the cap, which drives the saga's
/// compensation path.
#[derive(Default, Snapshot)]
pub struct Payment {
    pub entity: Entity,
    pub amount_cents: i64,
    pub status: String,
}

#[sourced(entity)]
impl Payment {
    #[event("PaymentCharged")]
    pub fn charge(&mut self, order_id: String, amount_cents: i64) {
        self.entity.set_id(&order_id);
        self.amount_cents = amount_cents;
        self.status = "charged".to_string();
    }

    #[event("PaymentDeclined")]
    pub fn decline(&mut self, order_id: String, reason: String) {
        self.entity.set_id(&order_id);
        self.status = format!("declined: {reason}");
    }
}
