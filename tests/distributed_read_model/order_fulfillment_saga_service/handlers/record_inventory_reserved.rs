use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::OutboxCommitExt;

use crate::fulfillment::{self, event, inventory_event, FulfillmentMsg};
use crate::order_fulfillment_saga_service::{OrderFulfillmentSaga, SagaRepo};

pub const COMMAND: &str = inventory_event::RESERVED;

pub fn guard(ctx: &Context<SagaRepo>) -> bool {
    ctx.has_fields(&["order_id"])
}

pub fn handle(ctx: &Context<SagaRepo>) -> Result<Value, HandlerError> {
    let msg = ctx.input::<FulfillmentMsg>()?;

    let mut saga: OrderFulfillmentSaga = ctx
        .repo()
        .get(&msg.order_id)?
        .ok_or_else(|| HandlerError::NotFound(msg.order_id.clone()))?;
    saga.inventory_reserved()?;

    let mut out = fulfillment::saga_event(
        event::INVENTORY_RESERVED,
        &FulfillmentMsg {
            order_id: msg.order_id.clone(),
            sku: saga.sku.clone(),
            quantity: saga.quantity,
            amount_cents: saga.amount_cents,
            ..Default::default()
        },
    );
    ctx.repo().outbox(&mut out).commit(&mut saga)?;

    Ok(json!({ "order_id": msg.order_id }))
}
