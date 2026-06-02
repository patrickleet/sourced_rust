use super::*;

pub const COMMAND: &str = "inventory.initialize";

pub fn guard(ctx: &Context<Repo>) -> bool {
    ctx.has_fields(&["sku", "stock"])
}

pub async fn handle(ctx: &Context<'_, Repo>) -> Result<Value, HandlerError> {
    let input = ctx.input::<InitInventoryInput>()?;

    let mut inv = Inventory::new();
    inv.initialize(input.sku.clone(), input.stock)?;

    ctx.repo().commit(&mut inv).await?;
    Ok(json!({ "sku": input.sku, "stock": input.stock }))
}
