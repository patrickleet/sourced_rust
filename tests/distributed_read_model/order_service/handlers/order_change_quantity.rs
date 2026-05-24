use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::{OutboxCommitExt, OutboxMessage};

use crate::order_service::{ChangeQuantity, Order, OrderRepo};

pub const COMMAND: &str = "order.change_quantity";

pub fn guard(ctx: &Context<OrderRepo>) -> bool {
    ctx.has_fields(&["id", "sku", "quantity"])
}

pub fn handle(ctx: &Context<OrderRepo>) -> Result<Value, HandlerError> {
    let input = ctx.input::<ChangeQuantity>()?;
    if input.quantity <= 0 {
        return Err(HandlerError::Rejected(
            "quantity must be positive".to_string(),
        ));
    }

    let mut order: Order = ctx
        .repo()
        .get(&input.id)?
        .ok_or_else(|| HandlerError::NotFound(input.id.clone()))?;
    order.change_quantity(input.sku.clone(), input.quantity)?;

    let mut outbox = OutboxMessage::domain_event("order.line_quantity_changed", &order)?;
    ctx.repo().outbox(&mut outbox).commit(&mut order)?;

    Ok(json!({ "id": input.id, "sku": input.sku }))
}
