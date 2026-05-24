use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::OutboxCommitExt;

use crate::fulfillment::{self, event, payment_event, FulfillmentMsg};
use crate::payment_service::PaymentRepo;

pub const COMMAND: &str = event::INVENTORY_RESERVED;

/// Amounts over this cap are declined (drives the compensation path).
const PAYMENT_CAP_CENTS: i64 = 100_000;

pub fn guard(ctx: &Context<PaymentRepo>) -> bool {
    ctx.has_fields(&["order_id", "amount_cents"])
}

pub fn handle(ctx: &Context<PaymentRepo>) -> Result<Value, HandlerError> {
    let msg = ctx.input::<FulfillmentMsg>()?;
    let mut payment = ctx.repo().get(&msg.order_id)?.unwrap_or_default();

    let mut out = if msg.amount_cents <= PAYMENT_CAP_CENTS {
        payment.charge(msg.order_id.clone(), msg.amount_cents)?;
        fulfillment::domain_event(
            payment_event::SUCCEEDED,
            &FulfillmentMsg {
                order_id: msg.order_id.clone(),
                ..Default::default()
            },
        )
    } else {
        payment.decline(msg.order_id.clone(), "amount over limit".to_string())?;
        fulfillment::domain_event(
            payment_event::DECLINED,
            &FulfillmentMsg {
                order_id: msg.order_id.clone(),
                detail: "amount over limit".to_string(),
                ..Default::default()
            },
        )
    };
    ctx.repo().outbox(&mut out).commit(&mut payment)?;

    Ok(json!({ "order_id": msg.order_id }))
}
