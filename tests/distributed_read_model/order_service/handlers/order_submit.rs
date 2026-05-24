use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::{OutboxCommitExt, OutboxMessage};

use crate::order_service::{Order, OrderRepo, SubmitOrder};

pub const COMMAND: &str = "order.submit";

pub fn guard(ctx: &Context<OrderRepo>) -> bool {
    ctx.has_fields(&["id"])
}

pub fn handle(ctx: &Context<OrderRepo>) -> Result<Value, HandlerError> {
    let input = ctx.input::<SubmitOrder>()?;

    let mut order: Order = ctx
        .repo()
        .get(&input.id)?
        .ok_or_else(|| HandlerError::NotFound(input.id.clone()))?;
    order.submit()?;

    let mut outbox = OutboxMessage::domain_event("order.submitted", &order)?;
    ctx.repo().outbox(&mut outbox).commit(&mut order)?;

    Ok(json!({ "id": input.id, "status": order.status }))
}
