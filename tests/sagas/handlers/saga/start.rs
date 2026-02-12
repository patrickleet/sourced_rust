use super::*;

pub const COMMAND: &str = "StartSaga";

pub fn guard(ctx: &Context<Repo>) -> bool {
    ctx.has_fields(&["saga_id", "order_id", "customer_id", "items", "total_cents"])
}

pub fn handle(ctx: &Context<Repo>) -> Result<Value, HandlerError> {
    let input = ctx.input::<StartSagaInput>()?;

    let mut saga = OrderFulfillmentSaga::new();
    saga.start(
        input.saga_id.clone(),
        input.order_id.clone(),
        input.customer_id.clone(),
        input.items.clone(),
        input.total_cents,
    );

    let mut msg = json_outbox_to(
        &format!("{}-create-order", input.saga_id),
        "CreateOrder",
        "orders",
        &CreateOrderMsg {
            saga_id: input.saga_id.clone(),
            order_id: input.order_id,
            customer_id: input.customer_id,
            items: input.items,
            total_cents: input.total_cents,
        },
    );

    ctx.repo().outbox(&mut msg).commit(&mut saga)?;
    Ok(json!({ "saga_id": input.saga_id }))
}
