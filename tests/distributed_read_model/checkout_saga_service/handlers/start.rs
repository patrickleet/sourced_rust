use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};

use crate::checkout::{
    checkout_command, checkout_event, json_outbox_event, CheckoutStarted, StartCheckout,
};
use crate::checkout_saga_service::{CheckoutRepo, CheckoutSaga};

pub const COMMAND: &str = checkout_command::START;

pub fn guard(ctx: &Context<CheckoutRepo>) -> bool {
    ctx.has_fields(&["checkout_id", "seat_id", "seat_category"])
}

pub async fn handle(ctx: &Context<'_, CheckoutRepo>) -> Result<Value, HandlerError> {
    let msg = ctx.input::<StartCheckout>()?;
    let mut saga = CheckoutSaga::default();
    saga.start(
        msg.checkout_id.clone(),
        msg.seat_id.clone(),
        msg.seat_category.clone(),
    )?;

    let event = CheckoutStarted {
        checkout_id: msg.checkout_id.clone(),
        seat_id: msg.seat_id.clone(),
        seat_category: msg.seat_category.clone(),
    };
    let out = json_outbox_event(&msg.checkout_id, checkout_event::STARTED, &event)?;
    ctx.repo().outbox(out).commit(&mut saga).await?;

    Ok(json!({ "checkout_id": msg.checkout_id }))
}
