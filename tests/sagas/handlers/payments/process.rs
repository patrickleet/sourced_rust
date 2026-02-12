use super::*;

pub const COMMAND: &str = "ProcessPayment";

pub fn guard(ctx: &Context<Repo>) -> bool {
    ctx.has_fields(&["saga_id", "order_id", "amount_cents"])
}

pub fn handle(ctx: &Context<Repo>) -> Result<Value, HandlerError> {
    let input = ctx.input::<ProcessPaymentMsg>()?;

    let payment_id = format!("pay-{}", input.order_id);
    let mut payment = Payment::new();
    payment.initiate(payment_id.clone(), input.order_id.clone(), input.amount_cents);
    payment.authorize("txn-distributed-001".to_string());
    payment.capture();

    let mut msg = json_outbox_to(
        &format!("{}-payment-succeeded", input.order_id),
        "PaymentSucceeded",
        "saga",
        &PaymentSucceededMsg {
            saga_id: input.saga_id,
            order_id: input.order_id,
        },
    );

    ctx.repo().outbox(&mut msg).commit(&mut payment)?;
    Ok(json!({ "payment_id": payment_id }))
}
