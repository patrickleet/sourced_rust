use super::*;

pub const COMMAND: &str = "payment.process";

pub fn guard(ctx: &Context<Repo>) -> bool {
    ctx.has_fields(&["saga_id", "order_id", "amount_cents"])
}

pub async fn handle(ctx: &Context<'_, Repo>) -> Result<Value, HandlerError> {
    let input = ctx.input::<ProcessPaymentMsg>()?;

    let payment_id = format!("pay-{}", input.order_id);
    let mut payment = Payment::new();
    payment.initiate(
        payment_id.clone(),
        input.order_id.clone(),
        input.amount_cents,
    )?;
    payment.authorize("txn-distributed-001".to_string())?;
    payment.capture()?;

    let msg = json_outbox_to(
        &format!("{}-payment-succeeded", input.order_id),
        "payment.succeeded",
        "saga",
        &PaymentSucceededMsg {
            saga_id: input.saga_id,
            order_id: input.order_id,
        },
    )?;

    ctx.repo().outbox(msg).commit(&mut payment).await?;
    Ok(json!({ "payment_id": payment_id }))
}
