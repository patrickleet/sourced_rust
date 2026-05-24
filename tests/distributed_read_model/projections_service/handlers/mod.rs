//! Projection handlers. Commands and events are both just messages, so the
//! projection service dispatches each event by type to the handler that owns the
//! matching read-model rows.

pub mod fulfillment;
pub mod order;
pub mod product;

use serde::{Deserialize, Serialize};
use sourced_rust::bus::Event;
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::{InMemoryReadModelStore, ReadModelError};

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

/// Every event type any projection handler consumes.
pub fn event_types() -> Vec<&'static str> {
    let mut types = Vec::new();
    types.extend_from_slice(product::EVENTS);
    types.extend_from_slice(order::EVENTS);
    types.extend_from_slice(fulfillment::EVENTS);
    types
}

pub fn event(ctx: &Context<InMemoryReadModelStore>) -> Result<Event, HandlerError> {
    Ok(ctx.input::<ProjectionMessage>()?.into())
}

pub fn read_model_error(err: ReadModelError) -> HandlerError {
    HandlerError::Repository(err.into())
}
