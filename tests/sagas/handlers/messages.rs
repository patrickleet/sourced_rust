//! Shared message types for inter-service communication.
//!
//! Each message is serialized as JSON via [`json_outbox_to`] so that
//! `microsvc::dispatch_event` can deserialize it on the receiving end.

use serde::{Deserialize, Serialize};
use sourced_rust::OutboxMessage;

use crate::order::OrderItem;

/// Create a JSON-serialized outbox message routed to a destination queue.
///
/// `OutboxMessage::encode_to` uses bitcode internally, but the microsvc
/// transport layer (`dispatch_event`) expects JSON.  This helper bridges the
/// gap by using `serde_json::to_vec` + `OutboxMessage::create_to`.
pub fn json_outbox_to<T: Serialize>(
    id: &str,
    event_type: &str,
    destination: &str,
    payload: &T,
) -> OutboxMessage {
    let bytes = serde_json::to_vec(payload).expect("JSON serialization should not fail");
    OutboxMessage::create_to(id, event_type, destination, bytes)
}

// === Saga → Order Service ===

#[derive(Serialize, Deserialize)]
pub struct CreateOrderMsg {
    pub saga_id: String,
    pub order_id: String,
    pub customer_id: String,
    pub items: Vec<OrderItem>,
    pub total_cents: u32,
}

#[derive(Serialize, Deserialize)]
pub struct CompleteOrderMsg {
    pub saga_id: String,
    pub order_id: String,
}

// === Order Service → Saga ===

#[derive(Serialize, Deserialize)]
pub struct OrderCreatedMsg {
    pub saga_id: String,
    pub order_id: String,
}

#[derive(Serialize, Deserialize)]
pub struct OrderCompletedMsg {
    pub saga_id: String,
    pub order_id: String,
}

// === Saga → Inventory Service ===

#[derive(Serialize, Deserialize)]
pub struct ReserveInventoryMsg {
    pub saga_id: String,
    pub order_id: String,
    pub sku: String,
    pub quantity: u32,
}

// === Inventory Service → Saga ===

#[derive(Serialize, Deserialize)]
pub struct InventoryReservedMsg {
    pub saga_id: String,
    pub order_id: String,
}

// === Saga → Payment Service ===

#[derive(Serialize, Deserialize)]
pub struct ProcessPaymentMsg {
    pub saga_id: String,
    pub order_id: String,
    pub amount_cents: u32,
}

// === Payment Service → Saga ===

#[derive(Serialize, Deserialize)]
pub struct PaymentSucceededMsg {
    pub saga_id: String,
    pub order_id: String,
}

// === Command Inputs ===

#[derive(Serialize, Deserialize)]
pub struct StartSagaInput {
    pub saga_id: String,
    pub order_id: String,
    pub customer_id: String,
    pub items: Vec<OrderItem>,
    pub total_cents: u32,
}

#[derive(Serialize, Deserialize)]
pub struct InitInventoryInput {
    pub sku: String,
    pub stock: u32,
}
