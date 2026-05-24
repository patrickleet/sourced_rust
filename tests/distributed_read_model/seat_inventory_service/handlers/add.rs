use serde_json::{json, Value};
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::OutboxCommitExt;

use crate::checkout::{json_outbox_event, seat_command, seat_event, AddSeat, SeatAdded};
use crate::seat_inventory_service::{Seat, SeatRepo};

pub const COMMAND: &str = seat_command::ADD;

pub fn guard(ctx: &Context<SeatRepo>) -> bool {
    ctx.has_fields(&["seat_id", "category"])
}

pub fn handle(ctx: &Context<SeatRepo>) -> Result<Value, HandlerError> {
    let msg = ctx.input::<AddSeat>()?;
    let mut seat = Seat::default();
    seat.add(msg.seat_id.clone(), msg.category.clone())?;

    let event = SeatAdded {
        seat_id: msg.seat_id.clone(),
        category: msg.category.clone(),
    };
    let mut out = json_outbox_event(&msg.seat_id, seat_event::ADDED, &event)?;
    ctx.repo().outbox(&mut out).commit(&mut seat)?;

    Ok(json!({ "seat_id": msg.seat_id }))
}
