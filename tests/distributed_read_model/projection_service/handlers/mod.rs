//! Projection handlers. The projection service dispatches each domain event by
//! event type to the handler that owns the matching read-model rows.

pub mod checkout;
pub mod seat;

use serde::{Deserialize, Serialize};
use sourced_rust::bus::Event;
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::ReadModelError;

use crate::projection_service::ProjectionDependencies;

#[derive(Debug, Deserialize, Serialize)]
pub struct ProjectionMessage {
    pub id: String,
    pub event_type: String,
    pub payload: Vec<u8>,
    pub metadata: Option<Vec<(String, String)>>,
}

impl From<&Event> for ProjectionMessage {
    fn from(event: &Event) -> Self {
        Self {
            id: event.id.clone(),
            event_type: event.event_type.clone(),
            payload: event.payload.clone(),
            metadata: event.metadata.clone(),
        }
    }
}

impl From<ProjectionMessage> for Event {
    fn from(message: ProjectionMessage) -> Self {
        Self {
            id: message.id,
            event_type: message.event_type,
            payload: message.payload,
            metadata: message.metadata,
        }
    }
}

pub fn event(ctx: &Context<ProjectionDependencies>) -> Result<Event, HandlerError> {
    Ok(ctx.input::<ProjectionMessage>()?.into())
}

pub fn read_model_error(err: ReadModelError) -> HandlerError {
    HandlerError::Repository(err.into())
}
