use sourced_rust::{sourced, Entity, Snapshot};

/// Stock per SKU. The saga reserves on the way in and releases on compensation.
#[derive(Default, Snapshot)]
pub struct Inventory {
    pub entity: Entity,
    pub available: i64,
    pub reserved: i64,
}

#[sourced(entity)]
impl Inventory {
    #[event("StockSet")]
    pub fn set_stock(&mut self, sku: String, quantity: i64) {
        self.entity.set_id(&sku);
        self.available = quantity;
        self.reserved = 0;
    }

    #[event("StockReserved", when = self.available >= quantity)]
    pub fn reserve(&mut self, quantity: i64) {
        self.available -= quantity;
        self.reserved += quantity;
    }

    #[event("StockReleased")]
    pub fn release(&mut self, quantity: i64) {
        self.available += quantity;
        self.reserved = (self.reserved - quantity).max(0);
    }
}
