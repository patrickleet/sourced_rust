use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::{OutboxCommitExt, OutboxMessage};

use crate::order_service::{Order, OrderRepo, RemoveLine};

pub const COMMAND: &str = "order.remove_line";

pub fn guard(ctx: &Context<OrderRepo>) -> bool {
    ctx.has_fields(&["id", "sku"])
}

pub fn handle(ctx: &Context<OrderRepo>) -> Result<Value, HandlerError> {
    let input = ctx.input::<RemoveLine>()?;

    let mut order: Order = ctx
        .repo()
        .get(&input.id)?
        .ok_or_else(|| HandlerError::NotFound(input.id.clone()))?;
    if order.status.as_str() != "open" {
        return Err(HandlerError::Rejected("order is not open".to_string()));
    }

    order.remove_line(input.sku.clone())?;

    let mut outbox = OutboxMessage::domain_event("order.line_removed", &order)?;
    ctx.repo().outbox(&mut outbox).commit(&mut order)?;

    Ok(json!({ "id": input.id, "sku": input.sku }))
}
