//! Reacts to the saga's `fulfillment.confirm_order` decision: transitions the
//! `Order` aggregate and publishes `order.confirmed` (a bitcode snapshot) so the
//! order projector updates `OrderView.status`.

use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::{OutboxCommitExt, OutboxMessage};

use crate::fulfillment::{event, FulfillmentMsg};
use crate::order_service::{Order, OrderRepo};

pub const COMMAND: &str = event::CONFIRM_ORDER;

pub fn guard(ctx: &Context<OrderRepo>) -> bool {
    ctx.has_fields(&["order_id"])
}

pub fn handle(ctx: &Context<OrderRepo>) -> Result<Value, HandlerError> {
    let msg = ctx.input::<FulfillmentMsg>()?;

    let mut order: Order = ctx
        .repo()
        .get(&msg.order_id)?
        .ok_or_else(|| HandlerError::NotFound(msg.order_id.clone()))?;
    order.confirm()?;

    let mut out = OutboxMessage::domain_event("order.confirmed", &order)?;
    ctx.repo().outbox(&mut out).commit(&mut order)?;

    Ok(json!({ "order_id": msg.order_id }))
}
