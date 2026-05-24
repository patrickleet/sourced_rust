//! Shared fulfillment-saga message contract.
//!
//! Saga steps are pub/sub JSON domain events (no outbox destination, fan-out via
//! the shared log). The orchestrator decides the next step; inventory/payment/
//! order services react and report. Each handler publishes exactly one next
//! event, so a single outbox message per commit is enough.

use serde::{Deserialize, Serialize};
use sourced_rust::OutboxMessage;

pub mod event {
    pub const REQUESTED: &str = "fulfillment.requested";
    pub const RESERVE_INVENTORY: &str = "fulfillment.reserve_inventory";
    pub const INVENTORY_RESERVED: &str = "fulfillment.inventory_reserved";
    pub const CHARGE_PAYMENT: &str = "fulfillment.charge_payment";
    pub const PAYMENT_SUCCEEDED: &str = "fulfillment.payment_succeeded";
    pub const PAYMENT_DECLINED: &str = "fulfillment.payment_declined";
    pub const RELEASE_INVENTORY: &str = "fulfillment.release_inventory";
    pub const INVENTORY_RELEASED: &str = "fulfillment.inventory_released";
    pub const CONFIRM_ORDER: &str = "fulfillment.confirm_order";
    pub const CANCEL_ORDER: &str = "fulfillment.cancel_order";
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

/// Build a pub/sub (no-destination) JSON fulfillment event for the outbox.
pub fn fulfillment_event(event_type: &str, msg: &FulfillmentMsg) -> OutboxMessage {
    let id = format!("{}:{}", msg.order_id, event_type);
    let payload = serde_json::to_vec(msg).expect("fulfillment message should encode");
    OutboxMessage::create(id, event_type, payload).expect("fulfillment outbox should build")
}

/// Decode a fulfillment event payload.
pub fn decode(event: &sourced_rust::bus::Event) -> FulfillmentMsg {
    serde_json::from_slice(&event.payload).expect("fulfillment message should decode")
}

/// Build the `fulfillment.requested` bus event that kicks off the saga.
pub fn requested_event(msg: &FulfillmentMsg) -> sourced_rust::bus::Event {
    let payload = serde_json::to_vec(msg).expect("fulfillment message should encode");
    sourced_rust::bus::Event::new(
        format!("{}:{}", msg.order_id, event::REQUESTED),
        event::REQUESTED,
        payload,
    )
}
