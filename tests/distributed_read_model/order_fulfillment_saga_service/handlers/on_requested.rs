use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::OutboxCommitExt;

use crate::fulfillment::{self, event, FulfillmentMsg};
use crate::order_fulfillment_saga_service::{OrderFulfillmentSaga, SagaRepo};

pub const COMMAND: &str = event::REQUESTED;

pub fn guard(ctx: &Context<SagaRepo>) -> bool {
    ctx.has_fields(&["order_id", "sku", "quantity", "amount_cents"])
}

pub fn handle(ctx: &Context<SagaRepo>) -> Result<Value, HandlerError> {
    let msg = ctx.input::<FulfillmentMsg>()?;

    let mut saga = OrderFulfillmentSaga::default();
    saga.start(
        msg.order_id.clone(),
        msg.sku.clone(),
        msg.quantity,
        msg.amount_cents,
    )?;

    let mut out = fulfillment::fulfillment_event(
        event::RESERVE_INVENTORY,
        &FulfillmentMsg {
            order_id: msg.order_id.clone(),
            sku: msg.sku.clone(),
            quantity: msg.quantity,
            ..Default::default()
        },
    );
    ctx.repo().outbox(&mut out).commit(&mut saga)?;

    Ok(json!({ "order_id": msg.order_id }))
}
