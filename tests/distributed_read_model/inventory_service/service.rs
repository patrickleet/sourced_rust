use std::sync::Arc;

use sourced_rust::microsvc::Service;
use sourced_rust::{AggregateBuilder, HashMapRepository};

use super::{handlers, Inventory, InventoryRepo};

pub fn model_service(repo: InventoryRepo) -> Arc<Service<InventoryRepo>> {
    Arc::new(sourced_rust::register_handlers!(
        Service::new(repo),
        handlers::reserve,
        handlers::release,
    ))
}

/// Seed starting stock for a SKU before the service starts reacting.
pub fn seed_stock(store: &HashMapRepository, sku: &str, quantity: i64) {
    let repo = store.clone().aggregate::<Inventory>();
    let mut inventory = Inventory::default();
    inventory
        .set_stock(sku.to_string(), quantity)
        .expect("seed stock should record");
    repo.commit(&mut inventory)
        .expect("seed stock should commit");
}
