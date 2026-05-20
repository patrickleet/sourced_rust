use bitcode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use sourced_rust::{digest, Entity};

/// Represents a line item in an order
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Encode, Decode)]
pub struct OrderItem {
    pub sku: String,
    pub quantity: u32,
    pub price_cents: u32,
}

/// Order status
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderStatus {
    Pending,
    InventoryReserved,
    PaymentProcessed,
    Completed,
    Cancelled,
}

/// Order aggregate - represents a customer order
#[derive(Default)]
pub struct Order {
    pub entity: Entity,
    customer_id: String,
    items: Vec<OrderItem>,
    status: OrderStatus,
    total_cents: u32,
    failure_reason: Option<String>,
}

impl Default for OrderStatus {
    fn default() -> Self {
        OrderStatus::Pending
    }
}

#[allow(dead_code)]
impl Order {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn customer_id(&self) -> &str {
        &self.customer_id
    }

    pub fn items(&self) -> &[OrderItem] {
        &self.items
    }

    pub fn status(&self) -> OrderStatus {
        self.status
    }

    pub fn total_cents(&self) -> u32 {
        self.total_cents
    }

    pub fn failure_reason(&self) -> Option<&str> {
        self.failure_reason.as_deref()
    }

    #[digest("OrderCreated")]
    pub fn create(&mut self, id: String, customer_id: String, items: Vec<OrderItem>) {
        self.entity.set_id(&id);
        self.customer_id = customer_id;
        self.total_cents = items.iter().map(|i| i.price_cents * i.quantity).sum();
        self.items = items;
        self.status = OrderStatus::Pending;
    }

    #[digest("InventoryReserved", when = self.status == OrderStatus::Pending)]
    pub fn mark_inventory_reserved(&mut self) {
        self.status = OrderStatus::InventoryReserved;
    }

    #[digest("PaymentProcessed", when = self.status == OrderStatus::InventoryReserved)]
    pub fn mark_payment_processed(&mut self) {
        self.status = OrderStatus::PaymentProcessed;
    }

    #[digest("OrderCompleted", when = self.status == OrderStatus::PaymentProcessed)]
    pub fn complete(&mut self) {
        self.status = OrderStatus::Completed;
    }

    #[digest("OrderCancelled", when = self.status != OrderStatus::Completed && self.status != OrderStatus::Cancelled)]
    pub fn cancel(&mut self, reason: String) {
        self.status = OrderStatus::Cancelled;
        self.failure_reason = Some(reason);
    }

    pub fn snapshot(&self) -> OrderSnapshot {
        OrderSnapshot {
            id: self.entity.id().to_string(),
            customer_id: self.customer_id.clone(),
            items: self.items.clone(),
            status: self.status,
            total_cents: self.total_cents,
            failure_reason: self.failure_reason.clone(),
        }
    }
}

sourced_rust::aggregate!(Order, entity {
    "OrderCreated"(id, customer_id, items) => create,
    "InventoryReserved"() => mark_inventory_reserved(),
    "PaymentProcessed"() => mark_payment_processed(),
    "OrderCompleted"() => complete(),
    "OrderCancelled"(reason) => cancel,
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrderSnapshot {
    pub id: String,
    pub customer_id: String,
    pub items: Vec<OrderItem>,
    pub status: OrderStatus,
    pub total_cents: u32,
    pub failure_reason: Option<String>,
}
