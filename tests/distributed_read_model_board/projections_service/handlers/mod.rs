//! Projection handlers. Each event is a message dispatched by type to the
//! handler that owns the matching read-model rows.

pub mod board;

use sourced_rust::bus::Event;
use sourced_rust::microsvc::{Context, HandlerError, MessageEnvelope};

use crate::projections_service::ProjectionDependencies;

pub fn event(ctx: &Context<ProjectionDependencies>) -> Result<Event, HandlerError> {
    Ok(ctx.input::<MessageEnvelope>()?.into())
}
