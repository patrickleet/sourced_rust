use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::{InMemoryReadModelStore, ReadModelUnitOfWorkExt};

use crate::checkout::{seat_event, SeatAdded, SeatReserved, SEAT_AVAILABLE, SEAT_RESERVED};
use crate::projection_service::CHECKOUT_SCREEN_CONSUMER;
use crate::read_models::{CheckoutStepView, SeatView};

pub const EVENTS: &[&str] = &[seat_event::ADDED, seat_event::RESERVED];

pub fn guard(ctx: &Context<InMemoryReadModelStore>) -> bool {
    ctx.has_fields(&["id", "event_type", "payload"])
}

pub fn handle(ctx: &Context<InMemoryReadModelStore>) -> Result<Value, HandlerError> {
    let event = super::event(ctx)?;

    match event.event_type.as_str() {
        seat_event::ADDED => {
            let msg: SeatAdded = event
                .json_decode()
                .map_err(|err| HandlerError::DecodeFailed(format!("seat added: {err}")))?;
            let row = SeatView {
                seat_id: msg.seat_id,
                category: msg.category,
                status: SEAT_AVAILABLE.to_string(),
                checkout_id: String::new(),
            };

            let mut session = ctx.repo().session();
            session.save(&row).map_err(super::read_model_error)?;
            session.mark_processed(CHECKOUT_SCREEN_CONSUMER, &event.id);
            session.commit().map_err(super::read_model_error)?;
        }
        seat_event::RESERVED => {
            let msg: SeatReserved = event
                .json_decode()
                .map_err(|err| HandlerError::DecodeFailed(format!("seat reserved: {err}")))?;
            let seat = SeatView {
                seat_id: msg.seat_id.clone(),
                category: msg.seat_category,
                status: SEAT_RESERVED.to_string(),
                checkout_id: msg.checkout_id.clone(),
            };
            let step = CheckoutStepView {
                checkout_id: msg.checkout_id,
                step: "seat_reserved".to_string(),
                detail: "seat reserved".to_string(),
            };

            let mut session = ctx.repo().session();
            session.save(&seat).map_err(super::read_model_error)?;
            session.save(&step).map_err(super::read_model_error)?;
            session.mark_processed(CHECKOUT_SCREEN_CONSUMER, &event.id);
            session.commit().map_err(super::read_model_error)?;
        }
        other => return Err(HandlerError::UnknownCommand(other.to_string())),
    }

    Ok(json!({ "event_id": event.id }))
}
