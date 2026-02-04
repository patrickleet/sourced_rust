//! Event payloads for inter-service communication in sagas.

use bitcode::{Decode, Encode};
use serde::{Deserialize, Serialize};

use super::OrderItem;

/// Payload for OrderCreated event.
#[derive(Clone, Debug, Serialize, Deserialize, Encode, Decode)]
pub struct OrderCreatedPayload {
    pub order_id: String,
    pub customer_id: String,
    pub items: Vec<OrderItem>,
    pub total_cents: u32,
}

/// Payload for InventoryReserved event.
#[derive(Clone, Debug, Serialize, Deserialize, Encode, Decode)]
pub struct InventoryReservedPayload {
    pub order_id: String,
    pub sku: String,
    pub quantity: u32,
}

/// Payload for PaymentSucceeded event.
#[derive(Clone, Debug, Serialize, Deserialize, Encode, Decode)]
pub struct PaymentSucceededPayload {
    pub order_id: String,
    pub payment_id: String,
}
