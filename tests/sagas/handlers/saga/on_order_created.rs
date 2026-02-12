use super::*;

pub const COMMAND: &str = "OrderCreated";

pub fn guard(ctx: &Context<Repo>) -> bool {
    ctx.has_fields(&["saga_id", "order_id"])
}

pub fn handle(ctx: &Context<Repo>) -> Result<Value, HandlerError> {
    let input = ctx.input::<OrderCreatedMsg>()?;

    let mut saga = ctx
        .repo()
        .get(&input.saga_id)?
        .ok_or_else(|| HandlerError::NotFound(input.saga_id.clone()))?;

    let sku = saga.items()[0].sku.clone();
    let quantity = saga.items()[0].quantity;

    let mut msg = json_outbox_to(
        &format!("{}-reserve-inventory", input.saga_id),
        "ReserveInventory",
        "inventory",
        &ReserveInventoryMsg {
            saga_id: input.saga_id.clone(),
            order_id: input.order_id,
            sku,
            quantity,
        },
    );

    ctx.repo().outbox(&mut msg).commit(&mut saga)?;
    Ok(json!({ "next": "ReserveInventory" }))
}
