use serde::{Deserialize, Serialize};
use sourced_rust::{sourced, Entity, Snapshot};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OrderLineState {
    pub sku: String,
    pub product_id: String,
    pub unit_cents: i64,
    pub quantity: i64,
}

#[derive(Default, Snapshot)]
pub struct Order {
    pub entity: Entity,
    pub customer: String,
    pub status: String,
    pub lines: Vec<OrderLineState>,
}

#[sourced(entity)]
impl Order {
    #[event("OrderPlaced")]
    pub fn place(&mut self, id: String, customer: String) {
        self.entity.set_id(&id);
        self.customer = customer;
        self.status = "open".to_string();
    }

    #[event("LineAdded", when = self.status.as_str() == "open")]
    pub fn add_line(&mut self, sku: String, product_id: String, unit_cents: i64, quantity: i64) {
        if let Some(line) = self.lines.iter_mut().find(|line| line.sku == sku) {
            line.quantity += quantity;
            line.unit_cents = unit_cents;
        } else {
            self.lines.push(OrderLineState {
                sku,
                product_id,
                unit_cents,
                quantity,
            });
        }
    }

    #[event("LineQuantityChanged", when = quantity > 0)]
    pub fn change_quantity(&mut self, sku: String, quantity: i64) {
        if let Some(line) = self.lines.iter_mut().find(|line| line.sku == sku) {
            line.quantity = quantity;
        }
    }

    #[event("LineRemoved")]
    pub fn remove_line(&mut self, sku: String) {
        self.lines.retain(|line| line.sku != sku);
    }

    #[event("OrderSubmitted", when = self.status.as_str() == "open" && !self.lines.is_empty())]
    pub fn submit(&mut self) {
        self.status = "submitted".to_string();
    }

    #[event("OrderConfirmed", when = self.status.as_str() == "submitted")]
    pub fn confirm(&mut self) {
        self.status = "confirmed".to_string();
    }

    #[event("OrderCancelled", when = self.status.as_str() != "cancelled")]
    pub fn cancel(&mut self) {
        self.status = "cancelled".to_string();
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PlaceOrder {
    pub id: String,
    pub customer: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AddLine {
    pub id: String,
    pub sku: String,
    pub product_id: String,
    pub unit_cents: i64,
    pub quantity: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ChangeQuantity {
    pub id: String,
    pub sku: String,
    pub quantity: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RemoveLine {
    pub id: String,
    pub sku: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SubmitOrder {
    pub id: String,
}
