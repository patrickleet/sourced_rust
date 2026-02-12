use super::*;

pub const COMMAND: &str = "InitInventory";

pub fn guard(ctx: &Context<Repo>) -> bool {
    ctx.has_fields(&["sku", "stock"])
}

pub fn handle(ctx: &Context<Repo>) -> Result<Value, HandlerError> {
    let input = ctx.input::<InitInventoryInput>()?;

    let mut inv = Inventory::new();
    inv.initialize(input.sku.clone(), input.stock);

    ctx.repo().commit(&mut inv)?;
    Ok(json!({ "sku": input.sku, "stock": input.stock }))
}
