use super::*;

pub const COMMAND: &str = "OrderCompleted";

pub fn guard(ctx: &Context<Repo>) -> bool {
    ctx.has_fields(&["saga_id", "order_id"])
}

pub fn handle(ctx: &Context<Repo>) -> Result<Value, HandlerError> {
    let input = ctx.input::<OrderCompletedMsg>()?;

    let mut saga = ctx
        .repo()
        .get(&input.saga_id)?
        .ok_or_else(|| HandlerError::NotFound(input.saga_id.clone()))?;
    saga.complete();

    ctx.repo().commit(&mut saga)?;
    Ok(json!({ "saga_id": input.saga_id, "status": "completed" }))
}
