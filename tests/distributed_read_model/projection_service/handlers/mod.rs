//! Projection handlers. The projection service dispatches each domain event by
//! event type to the handler that owns the matching read-model rows.

pub mod checkout;
pub mod seat;

use sourced_rust::bus::Event;
use sourced_rust::microsvc::{Context, HandlerError, MessageEnvelope};
use sourced_rust::ReadModelError;

use crate::projection_service::ProjectionDependencies;

pub fn event(ctx: &Context<ProjectionDependencies>) -> Result<Event, HandlerError> {
    Ok(ctx.input::<MessageEnvelope>()?.into())
}

pub fn read_model_error(err: ReadModelError) -> HandlerError {
    HandlerError::Repository(err.into())
}
