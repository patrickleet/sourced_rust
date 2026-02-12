use super::*;

pub const COMMAND: &str = "PaymentSucceeded";

pub fn guard(ctx: &Context<Repo>) -> bool {
    ctx.has_fields(&["saga_id", "order_id"])
}

pub fn handle(ctx: &Context<Repo>) -> Result<Value, HandlerError> {
    let input = ctx.input::<PaymentSucceededMsg>()?;

    let mut saga = ctx
        .repo()
        .get(&input.saga_id)?
        .ok_or_else(|| HandlerError::NotFound(input.saga_id.clone()))?;
    saga.payment_succeeded();

    let mut msg = json_outbox_to(
        &format!("{}-complete-order", input.saga_id),
        "CompleteOrder",
        "orders",
        &CompleteOrderMsg {
            saga_id: input.saga_id.clone(),
            order_id: input.order_id,
        },
    );

    ctx.repo().outbox(&mut msg).commit(&mut saga)?;
    Ok(json!({ "next": "CompleteOrder" }))
}
