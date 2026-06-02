use super::*;

pub const COMMAND: &str = "inventory.reserve";

pub fn guard(ctx: &Context<Repo>) -> bool {
    ctx.has_fields(&["saga_id", "order_id", "sku", "quantity"])
}

pub async fn handle(ctx: &Context<'_, Repo>) -> Result<Value, HandlerError> {
    let input = ctx.input::<ReserveInventoryMsg>()?;

    let mut inv = ctx
        .repo()
        .get(&input.sku)
        .await?
        .ok_or_else(|| HandlerError::NotFound(input.sku.clone()))?;

    if !inv.can_reserve(input.quantity) {
        return Err(HandlerError::Rejected("insufficient stock".into()));
    }
    inv.reserve(input.order_id.clone(), input.quantity)?;

    let msg = json_outbox_to(
        &format!("{}-inventory-reserved", input.order_id),
        "inventory.reserved",
        "saga",
        &InventoryReservedMsg {
            saga_id: input.saga_id,
            order_id: input.order_id,
        },
    )?;

    ctx.repo().outbox(msg).commit(&mut inv).await?;
    Ok(json!({ "reserved": input.quantity }))
}
