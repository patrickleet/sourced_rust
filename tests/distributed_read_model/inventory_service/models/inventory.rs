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
    #[event("StockSet", when = quantity >= 0)]
    pub fn set_stock(&mut self, sku: String, quantity: i64) {
        self.entity.set_id(&sku);
        self.available = quantity;
        self.reserved = 0;
    }

    #[event("StockReserved", when = quantity > 0 && self.available >= quantity)]
    pub fn reserve(&mut self, quantity: i64) {
        self.available -= quantity;
        self.reserved += quantity;
    }

    #[event("StockReleased", when = quantity > 0 && self.reserved >= quantity)]
    pub fn release(&mut self, quantity: i64) {
        self.available += quantity;
        self.reserved -= quantity;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stocked_inventory() -> Inventory {
        let mut inventory = Inventory::default();
        inventory.set_stock("W".to_string(), 10).unwrap();
        inventory
    }

    #[test]
    fn set_stock_ignores_negative_quantity() {
        let mut inventory = Inventory::default();

        inventory.set_stock("W".to_string(), -1).unwrap();

        assert!(inventory.entity.id().is_empty());
        assert_eq!(inventory.available, 0);
        assert!(inventory.entity.events().is_empty());
    }

    #[test]
    fn reserve_ignores_non_positive_quantity() {
        let mut inventory = stocked_inventory();

        inventory.reserve(0).unwrap();

        assert_eq!(inventory.available, 10);
        assert_eq!(inventory.reserved, 0);
        assert_eq!(inventory.entity.events().len(), 1);
    }

    #[test]
    fn reserve_ignores_quantities_above_available() {
        let mut inventory = stocked_inventory();

        inventory.reserve(11).unwrap();

        assert_eq!(inventory.available, 10);
        assert_eq!(inventory.reserved, 0);
        assert_eq!(inventory.entity.events().len(), 1);
    }

    #[test]
    fn release_ignores_quantities_above_reserved() {
        let mut inventory = stocked_inventory();
        inventory.reserve(3).unwrap();

        inventory.release(4).unwrap();

        assert_eq!(inventory.available, 7);
        assert_eq!(inventory.reserved, 3);
        assert_eq!(inventory.entity.events().len(), 2);
    }

    #[test]
    fn release_subtracts_reserved_stock_without_clamping() {
        let mut inventory = stocked_inventory();
        inventory.reserve(3).unwrap();

        inventory.release(2).unwrap();

        assert_eq!(inventory.available, 9);
        assert_eq!(inventory.reserved, 1);
        assert_eq!(inventory.entity.events().len(), 3);
    }
}
