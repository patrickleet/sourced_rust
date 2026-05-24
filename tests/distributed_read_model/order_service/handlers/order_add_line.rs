use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::{OutboxCommitExt, OutboxMessage};

use crate::order_service::{AddLine, Order, OrderRepo};

pub const COMMAND: &str = "order.add_line";

pub fn guard(ctx: &Context<OrderRepo>) -> bool {
    ctx.has_fields(&["id", "sku", "product_id", "unit_cents", "quantity"])
}

pub fn handle(ctx: &Context<OrderRepo>) -> Result<Value, HandlerError> {
    let input = ctx.input::<AddLine>()?;
    if input.quantity <= 0 || input.unit_cents < 0 {
        return Err(HandlerError::Rejected("invalid line".to_string()));
    }

    let mut order: Order = ctx
        .repo()
        .get(&input.id)?
        .ok_or_else(|| HandlerError::NotFound(input.id.clone()))?;
    order.add_line(
        input.sku.clone(),
        input.product_id.clone(),
        input.unit_cents,
        input.quantity,
    )?;

    let mut outbox = OutboxMessage::domain_event("order.line_added", &order)?;
    ctx.repo().outbox(&mut outbox).commit(&mut order)?;

    Ok(json!({ "id": input.id, "sku": input.sku }))
}
