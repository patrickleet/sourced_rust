use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::{OutboxCommitExt, OutboxMessage};

use crate::order_service::{Order, OrderRepo, PlaceOrder};

pub const COMMAND: &str = "order.place";

pub fn guard(ctx: &Context<OrderRepo>) -> bool {
    ctx.has_fields(&["id", "customer"])
}

pub fn handle(ctx: &Context<OrderRepo>) -> Result<Value, HandlerError> {
    let input = ctx.input::<PlaceOrder>()?;
    if ctx.repo().peek(&input.id)?.is_some() {
        return Err(HandlerError::Rejected(format!(
            "order {} already exists",
            input.id
        )));
    }

    let mut order = Order::default();
    order.place(input.id.clone(), input.customer.clone())?;

    let mut outbox = OutboxMessage::domain_event("order.placed", &order)?;
    ctx.repo().outbox(&mut outbox).commit(&mut order)?;

    Ok(json!({ "id": input.id }))
}
