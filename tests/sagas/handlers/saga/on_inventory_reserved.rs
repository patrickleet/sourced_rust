use super::*;

pub const COMMAND: &str = "InventoryReserved";

pub fn guard(ctx: &Context<Repo>) -> bool {
    ctx.has_fields(&["saga_id", "order_id"])
}

pub fn handle(ctx: &Context<Repo>) -> Result<Value, HandlerError> {
    let input = ctx.input::<InventoryReservedMsg>()?;

    let mut saga = ctx
        .repo()
        .get(&input.saga_id)?
        .ok_or_else(|| HandlerError::NotFound(input.saga_id.clone()))?;
    saga.inventory_reserved();

    let mut msg = json_outbox_to(
        &format!("{}-process-payment", input.saga_id),
        "ProcessPayment",
        "payments",
        &ProcessPaymentMsg {
            saga_id: input.saga_id.clone(),
            order_id: input.order_id,
            amount_cents: saga.total_cents(),
        },
    );

    ctx.repo().outbox(&mut msg).commit(&mut saga)?;
    Ok(json!({ "next": "ProcessPayment" }))
}
