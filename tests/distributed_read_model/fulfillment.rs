//! Shared fulfillment-saga message contract.
//!
//! The saga starts from a command. Every cross-service notification after that
//! is a pub/sub JSON domain event on the shared broker.

use serde::{Deserialize, Serialize};
use sourced_rust::OutboxMessage;

pub mod command {
    pub const START: &str = "fulfillment.start";
}

pub mod event {
    pub const STARTED: &str = "fulfillment.started";
    pub const INVENTORY_RESERVED: &str = "fulfillment.inventory_reserved";
    pub const COMPLETED: &str = "fulfillment.completed";
    pub const COMPENSATING: &str = "fulfillment.compensating";
    pub const CANCELLED: &str = "fulfillment.cancelled";
}

pub mod inventory_event {
    pub const RESERVED: &str = "inventory.reserved";
    pub const RELEASED: &str = "inventory.released";
}

pub mod payment_event {
    pub const SUCCEEDED: &str = "payment.succeeded";
    pub const DECLINED: &str = "payment.declined";
}

/// Correlation payload carried through every fulfillment step.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FulfillmentMsg {
    pub order_id: String,
    #[serde(default)]
    pub sku: String,
    #[serde(default)]
    pub quantity: i64,
    #[serde(default)]
    pub amount_cents: i64,
    #[serde(default)]
    pub detail: String,
}

/// Build a pub/sub JSON saga domain event for the outbox.
pub fn saga_event(event_type: &str, msg: &FulfillmentMsg) -> OutboxMessage {
    domain_event(event_type, msg)
}

/// Build a pub/sub JSON domain event for the outbox.
pub fn domain_event(event_type: &str, msg: &FulfillmentMsg) -> OutboxMessage {
    let id = format!("{}:{}", msg.order_id, event_type);
    let payload = serde_json::to_vec(msg).expect("fulfillment message should encode");
    OutboxMessage::create(id, event_type, payload).expect("fulfillment outbox should build")
}

/// Decode a fulfillment event payload.
pub fn decode(event: &sourced_rust::bus::Event) -> FulfillmentMsg {
    serde_json::from_slice(&event.payload).expect("fulfillment message should decode")
}
