//! Projects saga progress into `order_fulfillment_steps` (a `has_many` child of
//! `OrderView`). Owns only the steps table — disjoint from the order handler, so
//! there is no optimistic-version contention on the order row.

use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::{InMemoryReadModelStore, ReadModelUnitOfWorkExt};

use crate::fulfillment::{self, event};
use crate::read_models::OrderFulfillmentStepView;

pub const CONSUMER: &str = "order-fulfillment-projection";
pub const EVENTS: &[&str] = &[
    event::STARTED,
    event::INVENTORY_RESERVED,
    event::COMPLETED,
    event::COMPENSATING,
    event::CANCELLED,
];

pub fn guard(ctx: &Context<InMemoryReadModelStore>) -> bool {
    ctx.has_fields(&["id", "event_type", "payload"])
}

pub fn handle(ctx: &Context<InMemoryReadModelStore>) -> Result<Value, HandlerError> {
    let evt = super::event(ctx)?;
    let msg = fulfillment::decode(&evt);
    let step = evt
        .event_type
        .strip_prefix("fulfillment.")
        .unwrap_or(&evt.event_type)
        .to_string();

    let row = OrderFulfillmentStepView {
        order_id: msg.order_id.clone(),
        step,
        detail: msg.detail.clone(),
    };

    let mut session = ctx.repo().session();
    session
        .save(&row)
        .map_err(super::read_model_error)?
        .mark_processed(CONSUMER, &evt.id);
    session.commit().map_err(super::read_model_error)?;

    Ok(json!({ "event_id": evt.id }))
}
