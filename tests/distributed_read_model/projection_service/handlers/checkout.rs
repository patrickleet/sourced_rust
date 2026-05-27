use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::ReadModelWorkspaceExt;

use crate::checkout::{
    checkout_event, CheckoutStarted, SeatReservationCompleted, CHECKOUT_SEAT_RESERVED,
    CHECKOUT_STARTED, RESERVING_SEAT_MESSAGE, SEAT_RESERVED_MESSAGE,
};
use crate::projection_service::{ProjectionDependencies, CHECKOUT_SCREEN_CONSUMER};
use crate::read_models::{CheckoutStepView, CheckoutView};

pub const EVENTS: &[&str] = &[
    checkout_event::STARTED,
    checkout_event::SEAT_RESERVATION_COMPLETED,
];

pub fn guard(ctx: &Context<ProjectionDependencies>) -> bool {
    ctx.has_fields(&["id", "event_type", "payload"])
}

pub fn handle(ctx: &Context<ProjectionDependencies>) -> Result<Value, HandlerError> {
    let event = super::event(ctx)?;

    match event.event_type.as_str() {
        checkout_event::STARTED => {
            let msg: CheckoutStarted = event
                .json_decode()
                .map_err(|err| HandlerError::DecodeFailed(format!("checkout started: {err}")))?;
            let checkout = CheckoutView {
                checkout_id: msg.checkout_id.clone(),
                seat_id: msg.seat_id.clone(),
                seat_category: msg.seat_category,
                status: CHECKOUT_STARTED.to_string(),
                screen_message: RESERVING_SEAT_MESSAGE.to_string(),
                steps: Vec::new(),
                seat: None,
            };
            let step = checkout_step(&msg.checkout_id, "started", "checkout started");

            let mut workspace = ctx.read_model_store().workspace();
            workspace
                .upsert(&checkout)
                .map_err(super::read_model_error)?;
            workspace.upsert(&step).map_err(super::read_model_error)?;
            workspace.mark_processed(CHECKOUT_SCREEN_CONSUMER, &event.id);
            workspace.commit().map_err(super::read_model_error)?;
        }
        checkout_event::SEAT_RESERVATION_COMPLETED => {
            let msg: SeatReservationCompleted = event.json_decode().map_err(|err| {
                HandlerError::DecodeFailed(format!("seat reservation completed: {err}"))
            })?;
            let checkout = CheckoutView {
                checkout_id: msg.checkout_id.clone(),
                seat_id: msg.seat_id,
                seat_category: msg.seat_category,
                status: CHECKOUT_SEAT_RESERVED.to_string(),
                screen_message: SEAT_RESERVED_MESSAGE.to_string(),
                steps: Vec::new(),
                seat: None,
            };
            let step = checkout_step(
                &msg.checkout_id,
                "seat_reservation_completed",
                "seat reservation completed",
            );

            let mut workspace = ctx.read_model_store().workspace();
            workspace
                .upsert(&checkout)
                .map_err(super::read_model_error)?;
            workspace.upsert(&step).map_err(super::read_model_error)?;
            workspace.mark_processed(CHECKOUT_SCREEN_CONSUMER, &event.id);
            workspace.commit().map_err(super::read_model_error)?;
        }
        other => return Err(HandlerError::UnknownCommand(other.to_string())),
    }

    Ok(json!({ "event_id": event.id }))
}

fn checkout_step(checkout_id: &str, step: &str, detail: &str) -> CheckoutStepView {
    CheckoutStepView {
        checkout_id: checkout_id.to_string(),
        step: step.to_string(),
        detail: detail.to_string(),
    }
}
