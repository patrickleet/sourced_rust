use super::*;

pub const COMMAND: &str = "CreateOrder";

pub fn guard(ctx: &Context<Repo>) -> bool {
    ctx.has_fields(&["saga_id", "order_id", "customer_id", "items"])
}

pub fn handle(ctx: &Context<Repo>) -> Result<Value, HandlerError> {
    let input = ctx.input::<CreateOrderMsg>()?;

    let mut order = Order::new();
    order.create(input.order_id.clone(), input.customer_id, input.items);

    let mut msg = json_outbox_to(
        &format!("{}-order-created", input.order_id),
        "OrderCreated",
        "saga",
        &OrderCreatedMsg {
            saga_id: input.saga_id,
            order_id: input.order_id.clone(),
        },
    );

    ctx.repo().outbox(&mut msg).commit(&mut order)?;
    Ok(json!({ "order_id": input.order_id }))
}
