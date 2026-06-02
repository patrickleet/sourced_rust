use super::*;

pub const COMMAND: &str = "order.initialize";

pub fn guard(ctx: &Context<Repo>) -> bool {
    ctx.has_fields(&["saga_id", "order_id", "customer_id", "items"])
}

pub async fn handle(ctx: &Context<'_, Repo>) -> Result<Value, HandlerError> {
    let input = ctx.input::<CreateOrderMsg>()?;

    let mut order = Order::new();
    order.create(input.order_id.clone(), input.customer_id, input.items)?;

    let msg = json_outbox_to(
        &format!("{}-order-created", input.order_id),
        "order.initialized",
        "saga",
        &OrderInitializedMsg {
            saga_id: input.saga_id,
            order_id: input.order_id.clone(),
        },
    )?;

    ctx.repo().outbox(msg).commit(&mut order).await?;
    Ok(json!({ "order_id": input.order_id }))
}
