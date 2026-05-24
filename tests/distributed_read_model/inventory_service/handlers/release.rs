use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::OutboxCommitExt;

use crate::fulfillment::{self, event, inventory_event, FulfillmentMsg};
use crate::inventory_service::InventoryRepo;

pub const COMMAND: &str = event::COMPENSATING;

pub fn guard(ctx: &Context<InventoryRepo>) -> bool {
    ctx.has_fields(&["order_id", "sku", "quantity"])
}

pub fn handle(ctx: &Context<InventoryRepo>) -> Result<Value, HandlerError> {
    let msg = ctx.input::<FulfillmentMsg>()?;
    if msg.quantity <= 0 {
        return Err(HandlerError::Rejected(
            "quantity must be positive".to_string(),
        ));
    }

    let mut inventory = ctx
        .repo()
        .get(&msg.sku)?
        .ok_or_else(|| HandlerError::NotFound(msg.sku.clone()))?;
    if inventory.reserved < msg.quantity {
        return Err(HandlerError::Rejected(
            "reserved stock must cover release".to_string(),
        ));
    }

    inventory.release(msg.quantity)?;

    let mut out = fulfillment::domain_event(
        inventory_event::RELEASED,
        &FulfillmentMsg {
            order_id: msg.order_id.clone(),
            ..Default::default()
        },
    );
    ctx.repo().outbox(&mut out).commit(&mut inventory)?;

    Ok(json!({ "order_id": msg.order_id }))
}
