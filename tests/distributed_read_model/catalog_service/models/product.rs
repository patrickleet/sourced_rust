use serde::{Deserialize, Serialize};
use sourced_rust::{sourced, Entity, Snapshot};

#[derive(Default, Snapshot)]
pub struct Product {
    pub entity: Entity,
    pub name: String,
    pub unit_cents: i64,
}

#[sourced(entity)]
impl Product {
    #[event("ProductAdded")]
    pub fn add(&mut self, id: String, name: String, unit_cents: i64) {
        self.entity.set_id(&id);
        self.name = name;
        self.unit_cents = unit_cents;
    }

    #[event("ProductRepriced", when = unit_cents > 0)]
    pub fn reprice(&mut self, unit_cents: i64) {
        self.unit_cents = unit_cents;
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AddProduct {
    pub id: String,
    pub name: String,
    pub unit_cents: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RepriceProduct {
    pub id: String,
    pub unit_cents: i64,
}
