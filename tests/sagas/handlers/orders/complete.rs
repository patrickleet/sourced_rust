use super::*;

pub const COMMAND: &str = "CompleteOrder";

pub fn guard(ctx: &Context<Repo>) -> bool {
    ctx.has_fields(&["saga_id", "order_id"])
}

pub fn handle(ctx: &Context<Repo>) -> Result<Value, HandlerError> {
    let input = ctx.input::<CompleteOrderMsg>()?;

    let mut order = ctx
        .repo()
        .get(&input.order_id)?
        .ok_or_else(|| HandlerError::NotFound(input.order_id.clone()))?;
    order.mark_inventory_reserved();
    order.mark_payment_processed();
    order.complete();

    let mut msg = json_outbox_to(
        &format!("{}-order-completed", input.order_id),
        "OrderCompleted",
        "saga",
        &OrderCompletedMsg {
            saga_id: input.saga_id,
            order_id: input.order_id.clone(),
        },
    );

    ctx.repo().outbox(&mut msg).commit(&mut order)?;
    Ok(json!({ "order_id": input.order_id }))
}
